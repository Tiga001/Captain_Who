use super::*;

#[test]
fn freezes_declared_attachment_input_without_persisting_library_paths() {
    let library = tempfile::tempdir().unwrap();
    let root = library.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("objects")).unwrap();
    std::fs::write(root.join("objects/campus.png"), b"campus-image").unwrap();
    std::fs::create_dir(root.join("scripts")).unwrap();
    std::fs::write(root.join("scripts/build.py"), "# managed builder\n").unwrap();
    let storage = Arc::new(StorageService::open(&root.join("storage.sqlite")).unwrap());
    let run_id = "run-runtime-input";
    record_materialized_builder(
        &storage,
        run_id,
        AgentCommandRuntimeProfile::Documents,
        "scripts/build.py",
    );
    let read_path = "@attachments/attachment-1/campus.png";
    let call = AgentToolCall {
        id: "tool-runtime-input".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "python scripts/build.py --output report.docx",
            "inputs": [{
                "mountPath": "images/campus.png",
                "path": read_path
            }]
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("attachment-input-test".to_string()),
                root_path: Some(root.to_string_lossy().to_string()),
            }),
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: Some(root.to_string_lossy().to_string()),
                conversation_id: Some("conversation-1".to_string()),
                project_id: None,
                conversation_attachments: vec![AgentAttachmentReference {
                    id: "attachment-1".to_string(),
                    conversation_id: "conversation-1".to_string(),
                    message_id: "message-1".to_string(),
                    project_id: None,
                    kind: AgentInputAttachmentKind::Image,
                    name: "campus.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                    size_bytes: 12,
                    read_path: read_path.to_string(),
                    storage_rel_path: "objects/campus.png".to_string(),
                    created_at: 1,
                }],
                project_attachments: Vec::new(),
            }),
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        })),
        test_binding(
            AgentCommandRuntimeProfile::Documents,
            AgentCommandRuntimeKind::Python,
            &[("python-docx", "1.2.0")],
        ),
    )
    .with_runtime_services(run_id.to_string(), Some(storage));

    let request = command_request_from_call(&context, &call).unwrap();
    assert_eq!(request.inputs.len(), 2);
    let image = request
        .inputs
        .iter()
        .find(|input| input.mount_path == "images/campus.png")
        .unwrap();
    assert_eq!(image.size_bytes, 12);
    assert_eq!(
        image.sha256,
        format!("{:x}", Sha256::digest(b"campus-image"))
    );
    validate_frozen_command_trace_args(&request, &call.args).unwrap();
    let serialized = serde_json::to_string(&request).unwrap();
    assert!(
        !serialized.contains(root.join("objects/campus.png").to_string_lossy().as_ref()),
        "the Host may freeze the workspace-owned destination, but must not persist the private attachment storage path"
    );
}

#[test]
fn pdf_saved_script_and_document_attachments_freeze_without_a_workspace() {
    let library = tempfile::tempdir().unwrap();
    let root = library.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("objects")).unwrap();
    let pdf = b"%PDF-1.4\nfixture\n";
    let script = b"from pathlib import Path\nprint(Path(__file__).name)\n";
    std::fs::write(root.join("objects/manual.pdf"), pdf).unwrap();
    std::fs::write(root.join("objects/edit.py"), script).unwrap();
    let pdf_read_path = "@attachments/pdf-1/manual.pdf";
    let script_read_path = "@attachments/script-1/edit.py";
    let attachments = [
        (
            "pdf-1",
            "manual.pdf",
            "application/pdf",
            pdf_read_path,
            "objects/manual.pdf",
            pdf.len() as u64,
        ),
        (
            "script-1",
            "edit.py",
            "text/x-python",
            script_read_path,
            "objects/edit.py",
            script.len() as u64,
        ),
    ]
    .into_iter()
    .map(
        |(id, name, mime_type, read_path, storage_rel_path, size_bytes)| AgentAttachmentReference {
            id: id.to_string(),
            conversation_id: "conversation-pdf".to_string(),
            message_id: "message-pdf".to_string(),
            project_id: None,
            kind: AgentInputAttachmentKind::File,
            name: name.to_string(),
            mime_type: Some(mime_type.to_string()),
            size_bytes,
            read_path: read_path.to_string(),
            storage_rel_path: storage_rel_path.to_string(),
            created_at: 1,
        },
    )
    .collect();
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-pdf".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: Some(root.to_string_lossy().into_owned()),
                conversation_id: Some("conversation-pdf".to_string()),
                project_id: None,
                conversation_attachments: attachments,
                project_attachments: Vec::new(),
            }),
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
    );
    let call = AgentToolCall {
        id: "pdf-script-command".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "python '$MYCOPILOT_INPUT_ROOT/edit.py' '$MYCOPILOT_INPUT_ROOT/manual.pdf'",
            "inputs": [
                {"path": script_read_path, "mountPath": "edit.py"},
                {"path": pdf_read_path, "mountPath": "manual.pdf"}
            ],
            "reason": "Edit and verify the attached PDF"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };

    let request = command_request_from_call(&context, &call).unwrap();
    assert!(request.cwd.is_none());
    assert_eq!(request.inputs.len(), 2);
    assert_eq!(
        request
            .runtime_binding
            .as_deref()
            .map(|binding| binding.profile),
        Some(AgentCommandRuntimeProfile::Pdf)
    );
    validate_frozen_command_trace_args(&request, &call.args).unwrap();
    assert!(!serde_json::to_string(&request)
        .unwrap()
        .contains(root.to_string_lossy().as_ref()));
}

#[test]
fn frozen_trace_argument_verifier_binds_every_command_authority_field() {
    let live_args = json!({
        "command": "node scripts/build.mjs --output outputs/report.xlsx",
        "cwd": "scripts/.",
        "reason": "build the reviewed workbook",
        "observe": {
            "kinds": ["office"],
            "expectedOutputs": [" outputs/report.xlsx "],
            "additionalRoots": []
        },
        "runtimeProfile": "spreadsheets"
    });
    let call = AgentToolCall {
        id: "tool-runtime-trace".to_string(),
        tool: "run_command".to_string(),
        args: live_args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("temp".to_string()),
                root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        })),
        test_binding(
            AgentCommandRuntimeProfile::Spreadsheets,
            AgentCommandRuntimeKind::Node,
            &[("exceljs", "4.4.0")],
        ),
    );
    let frozen = command_request_from_call(&context, &call).unwrap();
    let args = live_args;
    validate_frozen_command_trace_args(&frozen, &args).unwrap();

    let mut removed_timeout = args.clone();
    removed_timeout["timeoutMs"] = json!(15_000);
    let error = validate_frozen_command_trace_args(&frozen, &removed_timeout).unwrap_err();
    assert!(error.contains("arguments are invalid"));

    let mut tampered = Vec::new();
    for (field, value) in [
        ("command", json!("node scripts/other.mjs")),
        ("cwd", json!("other")),
        ("reason", json!("different authority")),
    ] {
        let mut candidate = args.clone();
        candidate[field] = value;
        tampered.push(candidate);
    }
    let mut observe = args.clone();
    observe["observe"]["expectedOutputs"] = json!(["outputs/other.xlsx"]);
    tampered.push(observe);
    let mut runtime_profile = args.clone();
    runtime_profile["runtimeProfile"] = json!("presentations");
    tampered.push(runtime_profile);
    let mut hidden_runtime = args.clone();
    hidden_runtime["runtime"] = json!({
        "provider": "managedArtifact",
        "kind": "node",
        "requiredPackages": [{"name": "exceljs", "version": "4.4.0"}]
    });
    tampered.push(hidden_runtime);
    let mut unknown = args;
    unknown["executable"] = json!("/tmp/untrusted-node");
    tampered.push(unknown);

    for candidate in tampered {
        assert!(
            validate_frozen_command_trace_args(&frozen, &candidate).is_err(),
            "tampered command ToolCall was accepted: {candidate}"
        );
    }
}

#[test]
fn rejects_unknown_top_level_runtime_authority_fields() {
    let call = AgentToolCall {
        id: "tool-runtime-unknown".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "node scripts/build.mjs",
            "executable": "/tmp/fake-node"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("temp".to_string()),
            root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
        }),
        attachment_library: None,
        permissions: AgentPermissions::default(),
    }));

    let error = command_request_from_call(&context, &call).unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn freezes_bounded_office_observation_hint_without_authority_fields() {
    let call = AgentToolCall {
        id: "tool-observe".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "node build.mjs",
            "observe": {
                "kinds": ["office"],
                "expectedOutputs": [" outputs/report.xlsx ", "outputs/report.xlsx"],
                "additionalRoots": ["outputs"]
            }
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("temp".to_string()),
            root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            ..Default::default()
        },
    }));

    let request = command_request_from_call(&context, &call).unwrap();
    let observe = request.observe.unwrap();
    assert_eq!(observe.kinds, [AgentCommandArtifactObservationKind::Office]);
    assert_eq!(observe.expected_outputs, ["outputs/report.xlsx"]);
    assert_eq!(observe.additional_roots, ["outputs"]);
}

#[test]
fn canonicalizes_multiline_commands_without_discarding_script_whitespace() {
    let raw = "\r\n  printf one\rprintf two\r\n";
    assert_eq!(
        sanitize_command(raw).unwrap(),
        "\n  printf one\nprintf two\n"
    );

    assert!(sanitize_command(" \r\n\t ")
        .unwrap_err()
        .to_string()
        .contains("不能为空"));
    assert!(sanitize_command("printf ok\0hidden")
        .unwrap_err()
        .to_string()
        .contains("空字符"));
}

#[test]
fn command_character_limit_matches_the_model_schema_at_both_boundaries() {
    let at_limit = "x".repeat(MAX_COMMAND_CHARS);
    assert_eq!(sanitize_command(&at_limit).unwrap(), at_limit);

    let above_limit = "x".repeat(MAX_COMMAND_CHARS + 1);
    let error = sanitize_command(&above_limit).unwrap_err();
    assert!(error.to_string().contains(&MAX_COMMAND_CHARS.to_string()));

    let definition = RunCommandTool.definition();
    assert_eq!(
        definition.input_schema["properties"]["command"]["maxLength"],
        json!(MAX_COMMAND_CHARS)
    );
}

#[test]
fn definition_exposes_the_workspace_dependent_cwd_contract() {
    let definition = RunCommandTool.definition();
    let cwd_description = definition.input_schema["properties"]["cwd"]["description"]
        .as_str()
        .unwrap();
    let schema_description = definition.input_schema["description"].as_str().unwrap();

    for contract in [&definition.description, schema_description, cwd_description] {
        assert!(contract.contains("When no workspace is selected"));
        assert!(contract.contains("cwd is mandatory"));
        for alias in ["@home", "@desktop", "@documents", "@downloads"] {
            assert!(contract.contains(alias));
        }
        assert!(contract.contains("@desktop/project-dir"));
        assert!(contract.contains("backend-recognized managed PDF command"));
        assert!(contract.contains("activated PDF Skill"));
        assert!(contract.contains("private working directory"));
        assert!(contract.contains("write scope"));
    }
    assert!(definition
        .description
        .contains("cwd may be omitted to use the workspace root"));
    assert!(definition
        .description
        .contains("Before the first ordinary call, inspect World State workspace.binding"));
    assert!(definition
        .description
        .contains("When no workspace is selected, the first call must include cwd"));
    assert!(definition.description.contains(
        "must not be omitted even when the command, executable, or arguments already use absolute paths"
    ));
    assert!(cwd_description.contains(
        "cannot be omitted even when the command, executable, or arguments already use absolute paths"
    ));
    assert!(cwd_description.contains("it also cannot be relative or `.`"));
    assert!(cwd_description.contains("normally the target file's parent"));
    assert_eq!(definition.input_schema["required"], json!(["command"]));
}

#[test]
fn definition_forbids_using_commands_as_an_alternate_file_writer() {
    let description = RunCommandTool.definition().description;

    assert!(description.contains(
        "Never use run_command, shell redirection, a heredoc, or an inline script as an alternate writer"
    ));
    assert!(description.contains("or to bypass file-write approval"));
    assert!(description.contains("use apply_patch action=apply for short Direct changes"));
    assert!(description.contains("apply_patch Staged actions for long or staged content"));
    assert!(description.contains("managed Skill/Builder workflows retain their narrower"));
}

#[test]
fn rejects_cwd_outside_workspace() {
    let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("temp".to_string()),
            root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            ..Default::default()
        },
    }));
    let error = sanitize_cwd(&context, Some("../outside".to_string())).unwrap_err();

    assert!(error.to_string().contains("路径不能包含"));
}

#[test]
fn no_workspace_requires_explicit_cwd_and_accepts_alias_with_full_write() {
    let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions {
            write: AgentWritePermission::All,
            ..Default::default()
        },
    }));

    assert!(sanitize_cwd(&context, None).is_err());
    let cwd = sanitize_cwd(&context, Some("@home".to_string()))
        .unwrap()
        .unwrap();
    assert!(std::path::Path::new(&cwd).is_absolute());
}

#[test]
fn classifies_common_risk_levels() {
    assert_eq!(
        classify_command_risk("git diff"),
        AgentCommandRiskLevel::Unknown
    );
    assert_eq!(
        classify_command_risk("git status"),
        AgentCommandRiskLevel::Unknown
    );
    assert_eq!(
        classify_command_risk("git remote -v"),
        AgentCommandRiskLevel::ReadOnly
    );
    assert_eq!(
        classify_command_risk("cargo test"),
        AgentCommandRiskLevel::WritesWorkspace
    );
    assert_eq!(
        classify_command_risk("pnpm install"),
        AgentCommandRiskLevel::Network
    );
    assert_eq!(
        classify_command_risk("rm -rf target"),
        AgentCommandRiskLevel::Destructive
    );
    assert_eq!(
        classify_command_risk("git branch scratch"),
        AgentCommandRiskLevel::WritesWorkspace
    );
    assert_eq!(
        classify_command_risk("find . -delete"),
        AgentCommandRiskLevel::Destructive
    );
}

#[test]
fn rejected_command_model_projection_preserves_user_decision_and_feedback_on_replay() {
    let tool = RunCommandTool;
    for feedback in [None, Some(""), Some(" \t\n"), Some("请改成只查看文件名。")] {
        for legacy in [false, true] {
            let mut value = json!({ "status": "rejected" });
            if !legacy {
                value["code"] = json!("command.approval_rejected");
                value["decisionBy"] = json!("user");
            }
            value[if legacy { "message" } else { "userFeedback" }] = json!(feedback);
            let raw = AgentToolResult {
                exact_archive_file: None,
                call_id: "rejected-command".to_string(),
                tool: "run_command".to_string(),
                ok: true,
                result: Some(value),
                error: None,
            };
            let expected_feedback = feedback.filter(|feedback| !feedback.trim().is_empty());
            let live = tool.model_projection(&raw);
            let projected = live.result.as_ref().unwrap();
            assert_eq!(projected["status"], "rejected");
            assert_eq!(projected["code"], "command.approval_rejected");
            assert_eq!(projected["decisionBy"], "user");
            assert_eq!(projected["executionAttempted"], false);
            assert_eq!(projected["retryable"], false);
            assert_eq!(projected["userFeedback"].as_str(), expected_feedback);
            assert_eq!(
                projected["retryPolicy"],
                if expected_feedback.is_some() {
                    "follow_user_feedback_without_repeating_same_call"
                } else {
                    "new_explicit_user_instruction_required"
                }
            );
            let guidance = projected["message"].as_str().unwrap();
            assert!(guidance.contains("The user rejected this command approval"));
            assert!(guidance.contains("The command was not executed"));
            assert!(guidance.contains("without a new explicit user instruction"));
            assert!(guidance.contains("same or an equivalent command"));
            assert!(guidance.contains("does not mean the command tool is unavailable"));
            assert_eq!(tool.model_projection(&live).result, live.result);
            for durable in [
                tool.trace_projection(&raw),
                tool.checkpoint_projection(&raw),
            ] {
                assert_eq!(tool.model_projection(&durable).result, live.result);
            }
        }
    }
}

#[test]
fn model_projection_keeps_only_actionable_capture_safety_metadata() {
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: "command-capture".to_string(),
        tool: "run_command".to_string(),
        ok: true,
        result: Some(json!({
            "exitCode": 0,
            "stdout": "preview",
            "stderr": "",
            "stdoutTruncated": true,
            "stderrTruncated": false,
            "stdoutPreviewTruncated": true,
            "stderrPreviewTruncated": false,
            "originalBytes": 70_000_000,
            "capturedBytes": 67_108_864,
            "omittedBytes": 2_891_136,
            "truncatedAtSource": true,
            "stopReason": "exact_text_capture_safety_limit",
            "stdoutOriginalBytes": 70_000_000,
            "stdoutCapturedBytes": 67_108_864,
            "stdoutOmittedBytes": 2_891_136,
            "stderrOriginalBytes": 0,
            "stderrCapturedBytes": 0,
            "stderrOmittedBytes": 0,
        })),
        error: None,
    };

    let projected = run_command_model_projection(&raw);
    let result = projected.result.unwrap();
    assert_eq!(result["originalBytes"], 70_000_000);
    assert_eq!(result["omittedBytes"], 2_891_136);
    assert_eq!(result["truncatedAtSource"], true);
    assert_eq!(result["stdoutOmittedBytes"], 2_891_136);
    assert_eq!(result["stdoutPreviewTruncated"], true);
    assert!(result.get("stderrPreviewTruncated").is_none());
    assert!(result.get("stdoutOriginalBytes").is_none());
    assert!(result.get("stderrOriginalBytes").is_none());
}

#[test]
fn model_projection_keeps_the_complete_running_session_receipt() {
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: "command-running".to_string(),
        tool: "run_command".to_string(),
        ok: true,
        result: Some(json!({
            "status": "running",
            "sessionId": "cmd_0123456789abcdef0123456789abcdef",
            "output": "server listening on port 3000",
            "startedAt": 1_725_000_000_000_i64,
            "latestSequence": 3,
            "outputTruncated": false,
            "hostPrivateField": "must not reach the model",
        })),
        error: None,
    };

    let projected = run_command_model_projection(&raw);
    let result = projected.result.unwrap();

    assert_eq!(result["status"], "running");
    assert_eq!(result["sessionId"], "cmd_0123456789abcdef0123456789abcdef");
    assert_eq!(result["output"], "server listening on port 3000");
    assert_eq!(result["startedAt"], 1_725_000_000_000_i64);
    assert_eq!(result["latestSequence"], 3);
    assert_eq!(result["outputTruncated"], false);
    assert!(result.get("hostPrivateField").is_none());
}

#[test]
fn model_projection_preserves_authoritative_exact_history_route() {
    let history_open = "hist_v1_authoritative_command_page";
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: "command-exited".to_string(),
        tool: "run_command".to_string(),
        ok: true,
        result: Some(json!({
            "status": "exited",
            "exitCode": 0,
            "stdout": "bounded preview",
            "historyOpen": history_open,
            "continueWith": {
                "tool": "conversation_history",
                "args": { "open": history_open }
            },
            "hostPrivateField": "must not reach the model"
        })),
        error: None,
    };

    let projected = run_command_model_projection(&raw);
    let result = projected.result.unwrap();

    assert_eq!(result["historyOpen"], history_open);
    assert_eq!(result["continueWith"]["tool"], "conversation_history");
    assert_eq!(result["continueWith"]["args"]["open"], history_open);
    assert!(result.get("hostPrivateField").is_none());
}

#[test]
fn model_projection_retains_routes_while_other_consumers_keep_audit_fields() {
    let history_open = "hist_v1_authoritative_command_page";
    let read_path = format!("artifact://sha256/{}", "a".repeat(64));
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: "command-persisted-model-projection".to_string(),
        tool: "run_command".to_string(),
        ok: true,
        result: Some(json!({
            "status": "exited",
            "command": "pdfinfo $MYCOPILOT_INPUT_ROOT/manual.pdf",
            "cwd": ".",
            "exitCode": 0,
            "stdout": "bounded preview",
            "stderr": "",
            "historyOpen": history_open,
            "continueWith": {
                "tool": "conversation_history",
                "args": { "open": history_open }
            },
            "outputs": [{
                "name": "manual.pdf",
                "kind": "document",
                "readPath": read_path,
                "mimeType": "application/pdf",
                "sizeBytes": 123,
                "sha256": "a".repeat(64)
            }],
            "inputFiles": [{
                "mountPath": "manual.pdf",
                "sourceKind": "attachment",
                "sizeBytes": 456,
                "sha256": "b".repeat(64)
            }],
            "runtime": {
                "schemaVersion": 1,
                "providerId": "managed-artifact-runtime",
                "profile": "pdf",
                "kind": "python",
                "runtimeFingerprint": "host-audit-only",
                "resolvedPackages": [{ "name": "pypdf", "version": "6.15.0" }]
            }
        })),
        error: None,
    };

    let tool = RunCommandTool;
    let live = tool.model_projection(&raw);
    let checkpoint = tool.checkpoint_projection(&raw);

    let model_result = live.result.as_ref().unwrap();
    assert_eq!(model_result["historyOpen"], history_open);
    assert_eq!(model_result["continueWith"]["args"]["open"], history_open);
    assert_eq!(model_result["outputs"][0]["readPath"], read_path);
    assert!(model_result.get("runtime").is_none());
    assert!(model_result.get("inputFiles").is_none());
    assert!(model_result["outputs"][0].get("sha256").is_none());

    let checkpoint_result = checkpoint.result.as_ref().unwrap();
    assert_eq!(checkpoint_result["runtime"]["profile"], "pdf");
    assert_eq!(
        checkpoint_result["inputFiles"][0]["mountPath"],
        "manual.pdf"
    );
    assert_eq!(checkpoint_result["outputs"][0]["sha256"], "a".repeat(64));

    for durable in [
        tool.trace_projection(&raw),
        tool.archive_projection(&raw),
        tool.event_projection(&raw),
    ] {
        let result = durable.result.as_ref().unwrap();
        assert_eq!(result["runtime"]["profile"], "pdf");
        assert_eq!(result["inputFiles"][0]["mountPath"], "manual.pdf");
        assert_eq!(result["outputs"][0]["sha256"], "a".repeat(64));
    }
}

#[test]
fn model_contract_routes_long_lived_commands_without_polling_loops() {
    let definition = RunCommandTool.definition();

    assert!(definition
        .description
        .contains("running is not final success"));
    assert!(definition
        .description
        .contains("Host owns process lifetime"));
    assert!(definition
        .description
        .contains("do not add a deadline merely to bound tool waiting or confirm startup"));
    assert!(definition
        .description
        .contains("GUI app or long-lived server"));
    assert!(definition
        .description
        .contains("command_session once with action=wait"));
    assert!(definition
        .description
        .contains("do not repeatedly poll or narrate waiting"));
}
