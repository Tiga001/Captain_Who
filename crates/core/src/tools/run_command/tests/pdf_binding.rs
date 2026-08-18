use super::*;

#[test]
fn model_schema_exposes_profiles_without_timeout_or_runtime_authority() {
    let definition = RunCommandTool.definition();
    let properties = definition.input_schema["properties"].as_object().unwrap();
    assert!(properties.contains_key("runtimeProfile"));
    assert!(!properties.contains_key("timeoutMs"));
    assert!(!properties.contains_key("runtime"));
    assert_eq!(properties["command"]["maxLength"], json!(MAX_COMMAND_CHARS));
    let input_item = &properties["inputs"]["items"];
    assert_eq!(
        properties["runtimeProfile"]["enum"],
        json!(["documents", "spreadsheets", "presentations"])
    );
    assert_eq!(input_item["required"], json!(["path"]));
    assert!(input_item["properties"]["path"].is_object());
    let input_path_description = input_item["properties"]["path"]["description"]
        .as_str()
        .unwrap();
    assert!(input_path_description.contains("image-artifact://"));
    assert!(input_path_description.contains("artifact://"));
    assert!(input_item["properties"]["mountPath"].is_object());
    assert!(input_item["properties"].get("source").is_none());
    let serialized = serde_json::to_string(&definition.input_schema).unwrap();
    for forbidden in [
        "requiredPackages",
        "managedArtifact",
        "pptxgenjs",
        "4.0.1",
        "3.12.0",
        "MYCOPILOT_INPUT_ROOT",
        "built-in PDF",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "model schema leaked backend runtime authority: {forbidden}"
        );
    }
}

pub(super) fn pdf_skill_session(source_id: &str) -> Arc<crate::skills::SkillResourceSession> {
    let skill_id = SkillId::parse(format!("{source_id}:{PDF_LOCAL_ID}")).unwrap();
    Arc::new(
        memory_resource_session_for_test(
            skill_id,
            SkillRevision::parse("pdf-test-revision").unwrap(),
            SkillSourceId::parse(source_id).unwrap(),
            vec![(
                "references/reading.md".to_string(),
                SkillResourceKind::Reference,
                b"test".to_vec(),
            )],
        )
        .unwrap(),
    )
}

pub(super) fn pdf_and_documents_skill_session() -> Arc<crate::skills::SkillResourceSession> {
    let session = pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID);
    let documents = memory_resource_session_for_test(
        SkillId::parse(format!(
            "{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{DOCUMENTS_LOCAL_ID}"
        ))
        .unwrap(),
        SkillRevision::parse("documents-test-revision").unwrap(),
        SkillSourceId::parse(APPLICATION_BUNDLED_SKILL_SOURCE_ID).unwrap(),
        vec![(
            "templates/builder.py".to_string(),
            SkillResourceKind::Other,
            b"test".to_vec(),
        )],
    )
    .unwrap();
    session.extend_from(&documents).unwrap();
    session
}

#[test]
fn only_exact_bundled_pdf_skill_binds_the_hidden_pdf_runtime() {
    let command = "python -c 'from pypdf import PdfWriter; print(PdfWriter)'";
    let binding = test_binding(
        AgentCommandRuntimeProfile::Pdf,
        AgentCommandRuntimeKind::Python,
        &[
            ("pdfplumber", "0.11.9"),
            ("pypdf", "6.15.0"),
            ("pypdfium2", "5.12.1"),
            ("reportlab", "4.4.9"),
        ],
    );
    let trusted = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-pdf".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions::default(),
        }))
        .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
        binding,
    );
    let call = AgentToolCall {
        id: "pdf-command".to_string(),
        tool: "run_command".to_string(),
        args: json!({"command": command, "reason": "Create a PDF"}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let request = command_request_from_call(&trusted, &call).unwrap();
    assert!(request.cwd.is_none());
    assert_eq!(
        request
            .runtime_binding
            .as_deref()
            .map(|binding| binding.profile),
        Some(AgentCommandRuntimeProfile::Pdf)
    );

    let untrusted = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-pdf".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
    }))
    .with_skill_resources(Some(pdf_skill_session("bundled:test")));
    assert_eq!(
        trusted_managed_pdf_profile(&untrusted, command, None).unwrap(),
        None
    );
}

#[test]
fn bundled_pdf_freezes_a_word_qa_read_path_for_info_and_render_without_model_inputs() {
    let workspace = tempfile::tempdir().unwrap();
    let filename = "word-work/静夜思-visual-qa.pdf";
    let bytes = b"%PDF-1.4\nworkspace fixture\n";
    std::fs::create_dir(workspace.path().join("word-work")).unwrap();
    std::fs::write(workspace.path().join(filename), bytes).unwrap();
    let binding = test_binding(
        AgentCommandRuntimeProfile::Pdf,
        AgentCommandRuntimeKind::Python,
        &[
            ("pdfplumber", "0.11.9"),
            ("pypdf", "6.15.0"),
            ("pypdfium2", "5.12.1"),
            ("reportlab", "4.4.9"),
        ],
    );
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-aspen".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("Aspen manual".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
        }))
        .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
        binding,
    );
    for (id, command) in [
        ("pdf-word-qa-info", format!("pdfinfo \"{filename}\"")),
        (
            "pdf-word-qa-render",
            format!("pdftoppm -f 1 -l 1 -png \"{filename}\" outputs/page"),
        ),
    ] {
        let call = AgentToolCall {
            id: id.to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": command,
                "reason": "Inspect the Word visual QA PDF"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let request = command_request_from_call(&context, &call).unwrap();
        assert!(request.cwd.is_none());
        assert_eq!(request.command, command);
        assert_eq!(request.inputs.len(), 1);
        assert_eq!(request.inputs[0].mount_path, filename);
        assert_eq!(
            request.inputs[0].source,
            AgentFileInputRef::Workspace {
                path: filename.to_string()
            }
        );
        assert_eq!(request.inputs[0].size_bytes, bytes.len() as u64);
        assert_eq!(
            request.inputs[0].sha256,
            format!("{:x}", Sha256::digest(bytes))
        );
        validate_frozen_command_trace_args(&request, &call.args).unwrap();
        assert!(!serde_json::to_string(&request)
            .unwrap()
            .contains(workspace.path().to_string_lossy().as_ref()));
    }
}

#[test]
fn bundled_pdf_merges_explicit_attachment_and_workspace_inputs_deterministically() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_name = "工作区 手册.pdf";
    let workspace_bytes = b"%PDF-1.4\nworkspace fixture\n";
    std::fs::write(workspace.path().join(workspace_name), workspace_bytes).unwrap();

    let library = tempfile::tempdir().unwrap();
    let library_root = library.path().canonicalize().unwrap();
    std::fs::create_dir(library_root.join("objects")).unwrap();
    let attachment_bytes = b"%PDF-1.4\nattachment fixture\n";
    std::fs::write(library_root.join("objects/attached.pdf"), attachment_bytes).unwrap();
    let attachment_read_path = "@attachments/attachment-pdf/attached.pdf";

    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-mixed-pdf-inputs".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("mixed PDF inputs".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: Some(library_root.to_string_lossy().into_owned()),
                conversation_id: Some("conversation-mixed-pdf-inputs".to_string()),
                project_id: None,
                conversation_attachments: vec![AgentAttachmentReference {
                    id: "attachment-pdf".to_string(),
                    conversation_id: "conversation-mixed-pdf-inputs".to_string(),
                    message_id: "message-mixed-pdf-inputs".to_string(),
                    project_id: None,
                    kind: AgentInputAttachmentKind::File,
                    name: "attached.pdf".to_string(),
                    mime_type: Some("application/pdf".to_string()),
                    size_bytes: attachment_bytes.len() as u64,
                    read_path: attachment_read_path.to_string(),
                    storage_rel_path: "objects/attached.pdf".to_string(),
                    created_at: 1,
                }],
                project_attachments: Vec::new(),
            }),
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
        }))
        .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
        test_binding(
            AgentCommandRuntimeProfile::Pdf,
            AgentCommandRuntimeKind::Python,
            &[
                ("pdfplumber", "0.11.9"),
                ("pypdf", "6.15.0"),
                ("pypdfium2", "5.12.1"),
                ("reportlab", "4.4.9"),
            ],
        ),
    );
    let call = AgentToolCall {
        id: "pdf-mixed-inputs".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": format!(
                "pdfinfo \"{workspace_name}\"; pdfinfo \"$MYCOPILOT_INPUT_ROOT/attached.pdf\""
            ),
            "inputs": [
                {"path": attachment_read_path, "mountPath": "attached.pdf"}
            ],
            "reason": "Compare the workspace and attached PDFs"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };

    let request = command_request_from_call(&context, &call).unwrap();
    assert_eq!(request.inputs.len(), 2);
    assert_eq!(request.inputs[0].mount_path, "attached.pdf");
    assert_eq!(request.inputs[1].mount_path, workspace_name);
    assert_eq!(
        request.inputs[1].source,
        AgentFileInputRef::Workspace {
            path: workspace_name.to_string()
        }
    );
    assert_eq!(
        request.inputs[0].sha256,
        format!("{:x}", Sha256::digest(attachment_bytes))
    );
    assert_eq!(
        request.inputs[1].sha256,
        format!("{:x}", Sha256::digest(workspace_bytes))
    );
    validate_frozen_command_trace_args(&request, &call.args).unwrap();
    let serialized = serde_json::to_string(&request).unwrap();
    assert!(!serialized.contains(workspace.path().to_string_lossy().as_ref()));
    assert!(!serialized.contains(library_root.to_string_lossy().as_ref()));
}

#[test]
fn bundled_pdf_missing_workspace_path_returns_an_inputs_recovery() {
    let workspace = tempfile::tempdir().unwrap();
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-missing-pdf".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("missing PDF".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
        }))
        .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
        test_binding(
            AgentCommandRuntimeProfile::Pdf,
            AgentCommandRuntimeKind::Python,
            &[
                ("pdfplumber", "0.11.9"),
                ("pypdf", "6.15.0"),
                ("pypdfium2", "5.12.1"),
                ("reportlab", "4.4.9"),
            ],
        ),
    );
    let call = AgentToolCall {
        id: "pdf-missing-info".to_string(),
        tool: "run_command".to_string(),
        args: json!({"command": "pdfinfo \"not-here.pdf\""}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };

    let error = command_request_from_call(&context, &call).unwrap_err();
    assert_eq!(error.code(), Some("agent.fileInput.notFound"));
    assert!(error.to_string().contains("run_command.inputs"));
    assert!(error.to_string().contains("$MYCOPILOT_INPUT_ROOT"));
}

#[test]
fn bundled_pdf_outputs_path_never_binds_a_same_named_workspace_file() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("outputs")).unwrap();
    std::fs::write(
        workspace.path().join("outputs/draft.pdf"),
        b"workspace decoy",
    )
    .unwrap();
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-private-pdf-output".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("PDF output isolation".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
        }))
        .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
        test_binding(
            AgentCommandRuntimeProfile::Pdf,
            AgentCommandRuntimeKind::Python,
            &[
                ("pdfplumber", "0.11.9"),
                ("pypdf", "6.15.0"),
                ("pypdfium2", "5.12.1"),
                ("reportlab", "4.4.9"),
            ],
        ),
    );
    let call = AgentToolCall {
        id: "pdf-private-output-info".to_string(),
        tool: "run_command".to_string(),
        args: json!({"command": "pdfinfo outputs/draft.pdf"}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };

    let request = command_request_from_call(&context, &call).unwrap();
    assert!(request.inputs.is_empty());
    assert_eq!(request.command, "pdfinfo outputs/draft.pdf");
    validate_frozen_command_trace_args(&request, &call.args).unwrap();
}

#[test]
fn bundled_pdf_explicit_external_input_requires_and_accepts_full_read() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let external = fixture.path().join("external manual.pdf");
    let bytes = b"%PDF-1.4\nexternal fixture\n";
    std::fs::write(&external, bytes).unwrap();
    // `tempfile` lives below `/var` on macOS, whose public spelling is a symlink to
    // `/private/var`. External input authority deliberately rejects symlink traversal, so
    // exercise the same canonical absolute path a real file picker/Host resolver provides.
    let external = external.canonicalize().unwrap();
    let binding = test_binding(
        AgentCommandRuntimeProfile::Pdf,
        AgentCommandRuntimeKind::Python,
        &[
            ("pdfplumber", "0.11.9"),
            ("pypdf", "6.15.0"),
            ("pypdfium2", "5.12.1"),
            ("reportlab", "4.4.9"),
        ],
    );
    let make_context = |read| {
        with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-external-pdf".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("external PDF".to_string()),
                    root_path: Some(workspace.to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..AgentPermissions::default()
                },
            }))
            .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
            binding.clone(),
        )
    };
    let call = AgentToolCall {
        id: "pdf-external-info".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "pdfinfo \"$MYCOPILOT_INPUT_ROOT/external.pdf\"",
            "inputs": [{
                "path": external.to_string_lossy(),
                "mountPath": "external.pdf"
            }]
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };

    let denied =
        command_request_from_call(&make_context(AgentReadPermission::WorkspaceOnly), &call)
            .unwrap_err();
    assert_eq!(denied.code(), Some("agent.fileInput.authorizationDenied"));

    let request =
        command_request_from_call(&make_context(AgentReadPermission::All), &call).unwrap();
    assert_eq!(request.inputs.len(), 1);
    assert_eq!(request.inputs[0].mount_path, "external.pdf");
    assert_eq!(
        request.inputs[0].source,
        AgentFileInputRef::External {
            path: external.to_string_lossy().into_owned()
        }
    );
    assert_eq!(request.inputs[0].size_bytes, bytes.len() as u64);
    assert_eq!(
        request.inputs[0].sha256,
        format!("{:x}", Sha256::digest(bytes))
    );
    validate_frozen_command_trace_args(&request, &call.args).unwrap();
}

#[test]
fn bundled_pdf_freezes_a_conversation_artifact_without_a_workspace() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let storage = Arc::new(StorageService::open(&root.join("storage.sqlite")).unwrap());
    let conversation_id = "conversation-pdf-artifact";
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "PDF Artifact input".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-pdf-artifact".to_string(),
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
    let source = root.join("generated-manual.pdf");
    let bytes = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";
    std::fs::write(&source, bytes).unwrap();
    let published = storage
        .publish_managed_artifact_file(
            &source,
            ManagedArtifactAuthority {
                conversation_id,
                run_id: "run-pdf-artifact-source",
                call_id: "call-pdf-artifact-source",
            },
        )
        .unwrap();
    let read_path = published.read_path();
    assert!(read_path.starts_with("artifact://sha256/"));

    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some(conversation_id.to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions::default(),
        }))
        .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
        test_binding(
            AgentCommandRuntimeProfile::Pdf,
            AgentCommandRuntimeKind::Python,
            &[
                ("pdfplumber", "0.11.9"),
                ("pypdf", "6.15.0"),
                ("pypdfium2", "5.12.1"),
                ("reportlab", "4.4.9"),
            ],
        ),
    )
    .with_runtime_services("run-pdf-artifact".to_string(), Some(storage));
    let call = AgentToolCall {
        id: "pdf-artifact-info".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "pdfinfo \"$MYCOPILOT_INPUT_ROOT/manual.pdf\"",
            "inputs": [{
                "path": read_path,
                "mountPath": "manual.pdf"
            }],
            "reason": "Inspect a PDF produced earlier in this conversation"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };

    let request = command_request_from_call(&context, &call).unwrap();
    assert!(request.cwd.is_none());
    assert_eq!(request.inputs.len(), 1);
    assert_eq!(request.inputs[0].mount_path, "manual.pdf");
    assert_eq!(request.inputs[0].size_bytes, bytes.len() as u64);
    assert_eq!(request.inputs[0].sha256, published.sha256);
    assert!(matches!(
        &request.inputs[0].source,
        AgentFileInputRef::GeneratedArtifact { uri, .. } if uri == &published.read_path()
    ));
    validate_frozen_command_trace_args(&request, &call.args).unwrap();
}
