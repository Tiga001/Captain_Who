use super::*;

#[test]
fn model_path_input_derives_a_private_mount_name_without_source_routing() {
    let resolved = resolve_run_command_inputs(
        &AgentFileInputExecutionContext::default(),
        vec![RunCommandModelPathInput {
            path: "assets/hero.png".to_string(),
            mount_path: None,
        }],
    )
    .unwrap();

    assert_eq!(
        resolved,
        vec![AgentFileInputSpec {
            mount_path: "hero.png".to_string(),
            source: crate::protocol::AgentFileInputRef::Workspace {
                path: "assets/hero.png".to_string(),
            },
        }]
    );
}

#[test]
fn builds_command_request_for_approval() {
    let call = AgentToolCall {
        id: "tool-1".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": " cargo test ",
            "cwd": "agent/rust",
            "reason": "verify tests"
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

    assert_eq!(request.id, "tool-1");
    assert_eq!(request.command, " cargo test ");
    assert_eq!(request.cwd.as_deref(), Some("agent/rust"));
    assert_eq!(request.timeout_ms, None);
    assert_eq!(request.approval_status, AgentApprovalStatus::Required);
    assert_eq!(
        request.risk_level,
        Some(AgentCommandRiskLevel::WritesWorkspace)
    );
    assert_eq!(request.reason.as_deref(), Some("verify tests"));
    assert!(request.observe.is_none());
    assert!(request.runtime_binding.is_none());
}

#[test]
fn omitted_timeout_means_no_hard_process_deadline() {
    let call = AgentToolCall {
        id: "tool-without-timeout".to_string(),
        tool: "run_command".to_string(),
        args: json!({ "command": "pwd" }),
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

    let request = command_request_from_call(&context, &call).unwrap();

    assert_eq!(request.timeout_ms, None);
    validate_frozen_command_trace_args(&request, &call.args).unwrap();
}

#[test]
fn live_model_arguments_reject_removed_timeout_field() {
    let call = AgentToolCall {
        id: "tool-with-live-timeout".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "pwd",
            "timeoutMs": 15_000
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

    assert!(error.to_string().contains("unknown field `timeoutMs`"));
}

#[test]
fn resolves_and_freezes_a_model_friendly_runtime_profile() {
    let call = AgentToolCall {
        id: "tool-runtime".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "node scripts/build.mjs --output outputs/report.xlsx",
            "runtimeProfile": "spreadsheets"
        }),
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

    let request = command_request_from_call(&context, &call).unwrap();
    let frozen = serde_json::to_value(&request).unwrap();
    assert!(frozen.get("runtime").is_none());
    assert_eq!(frozen["runtimeBinding"]["profile"], "spreadsheets");
    assert_eq!(
        frozen["runtimeBinding"]["providerId"],
        crate::artifact_runtime::ARTIFACT_RUNTIME_PROVIDER_ID
    );
    assert_eq!(frozen["runtimeBinding"]["kind"], "node");
    assert_eq!(
        frozen["runtimeBinding"]["resolvedPackages"][0]["version"],
        "4.4.0"
    );
    assert_eq!(frozen["runtimeBinding"]["runtimeVersion"], "22.23.1");
    assert!(frozen["runtimeBinding"]["runtimeFingerprint"]
        .as_str()
        .unwrap()
        .starts_with("artifact-runtime-sha256-v1:"));
}

#[test]
fn backend_binds_managed_builder_profile_and_observation_from_office_output() {
    struct Case {
        command: &'static str,
        script: &'static str,
        profile: AgentCommandRuntimeProfile,
        kind: AgentCommandRuntimeKind,
        packages: &'static [(&'static str, &'static str)],
    }
    let cases = [
        Case {
            command: "python scripts/build.py --output outputs/report.docx",
            script: "scripts/build.py",
            profile: AgentCommandRuntimeProfile::Documents,
            kind: AgentCommandRuntimeKind::Python,
            packages: &[("python-docx", "1.2.0")],
        },
        Case {
            command: "python scripts/build.py --output=outputs/report.xlsx",
            script: "scripts/build.py",
            profile: AgentCommandRuntimeProfile::Spreadsheets,
            kind: AgentCommandRuntimeKind::Python,
            packages: &[("openpyxl", "3.1.5")],
        },
        Case {
            command: "node scripts/build.mjs --output 'outputs/product intro.pptx'",
            script: "scripts/build.mjs",
            profile: AgentCommandRuntimeProfile::Presentations,
            kind: AgentCommandRuntimeKind::Node,
            packages: &[("pptxgenjs", "4.0.1")],
        },
    ];
    for case in cases {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join("outputs")).unwrap();
        let script_path = workspace.path().join(case.script);
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(&script_path, "# managed builder\n").unwrap();
        let storage =
            Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
        let run_id = format!("run-auto-{:?}", case.profile);
        record_materialized_builder(&storage, &run_id, case.profile, case.script);
        let args = json!({ "command": case.command });
        let call = AgentToolCall {
            id: format!("tool-auto-{:?}", case.profile),
            tool: "run_command".to_string(),
            args: args.clone(),
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
                    display_name: Some("managed-builder".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(case.profile, case.kind, case.packages),
        )
        .with_runtime_services(run_id, Some(storage));

        let request = command_request_from_call(&context, &call).unwrap();
        assert_eq!(
            request
                .runtime_binding
                .as_deref()
                .map(|binding| binding.profile),
            Some(case.profile)
        );
        let observe = request.observe.as_ref().expect("backend observation");
        assert_eq!(observe.kinds, [AgentCommandArtifactObservationKind::Office]);
        assert_eq!(observe.expected_outputs.len(), 1);
        assert!(observe.expected_outputs[0].starts_with("outputs/"));
        validate_frozen_command_trace_args(&request, &args).unwrap();
    }
}

#[test]
fn exact_bundled_builders_reject_source_and_python_editors_require_exactly_one_source() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("scripts")).unwrap();
    std::fs::create_dir_all(workspace.path().join("outputs")).unwrap();
    std::fs::write(workspace.path().join("source.xlsx"), b"PK\x03\x04source").unwrap();

    let storage = Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());

    let builder = "scripts/build.py";
    std::fs::write(workspace.path().join(builder), b"# fixed builder\n").unwrap();
    let builder_run_id = "run-excel-builder-source-contract";
    record_materialized_builder(
        &storage,
        builder_run_id,
        AgentCommandRuntimeProfile::Spreadsheets,
        builder,
    );
    let builder_context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("builder source contract".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        })),
        test_binding(
            AgentCommandRuntimeProfile::Spreadsheets,
            AgentCommandRuntimeKind::Python,
            &[("openpyxl", "3.1.5")],
        ),
    )
    .with_runtime_services(builder_run_id.to_string(), Some(storage.clone()));
    let builder_call = AgentToolCall {
        id: "tool-excel-builder-with-source".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": format!(
                "python {builder} --source source.xlsx --output outputs/new.xlsx"
            ),
            "inputs": [{ "path": "source.xlsx", "mountPath": "source.xlsx" }]
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let error = command_request_from_call(&builder_context, &builder_call)
        .expect_err("a fixed Builder must never be repurposed as an existing-file editor");
    assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));
    assert!(error.to_string().contains("Builder"));

    for (profile, editor, source, output) in [
        (
            AgentCommandRuntimeProfile::Documents,
            "scripts/edit_doc.py",
            "source.docx",
            "outputs/edited.docx",
        ),
        (
            AgentCommandRuntimeProfile::Spreadsheets,
            "scripts/edit_sheet.py",
            "source.xlsx",
            "outputs/edited.xlsx",
        ),
    ] {
        std::fs::write(workspace.path().join(editor), b"# fixed editor\n").unwrap();
        if !workspace.path().join(source).exists() {
            std::fs::write(workspace.path().join(source), b"PK\x03\x04source").unwrap();
        }
        let run_id = format!("run-{profile:?}-editor-source-contract");
        record_materialized_python_editor(&storage, &run_id, profile, editor);
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("editor source contract".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(
                profile,
                AgentCommandRuntimeKind::Python,
                match profile {
                    AgentCommandRuntimeProfile::Documents => &[("python-docx", "1.2.0")],
                    AgentCommandRuntimeProfile::Spreadsheets => &[("openpyxl", "3.1.5")],
                    _ => unreachable!(),
                },
            ),
        )
        .with_runtime_services(run_id, Some(storage.clone()));

        let missing_source = AgentToolCall {
            id: format!("tool-{profile:?}-editor-missing-source"),
            tool: "run_command".to_string(),
            args: json!({ "command": format!("python {editor} --output {output}") }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = command_request_from_call(&context, &missing_source)
            .expect_err("a fixed Editor must require exactly one source");
        assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));

        let valid = AgentToolCall {
            id: format!("tool-{profile:?}-editor-valid-source"),
            tool: "run_command".to_string(),
            args: json!({
                "command": format!(
                    "python {editor} --source {source} --output {output}"
                ),
                "inputs": [{ "path": source, "mountPath": source }]
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let request = command_request_from_call(&context, &valid)
            .expect("an exact fixed Editor with one frozen source is authorized");
        let transaction = request
            .managed_office_script
            .as_deref()
            .expect("the Editor receives a Host publication contract");
        assert_eq!(
            transaction.purpose,
            crate::office::OfficeManagedScriptPurpose::Edit
        );
        assert_eq!(transaction.source_mount_path.as_deref(), Some(source));
        assert_eq!(
            request.inputs.len(),
            2,
            "source and fixed script are frozen"
        );
        validate_frozen_command_trace_args(&request, &valid.args).unwrap();

        let logical_source = format!(
            "logical-source.{}",
            Path::new(source).extension().unwrap().to_string_lossy()
        );
        let aliased_source = AgentToolCall {
            id: format!("tool-{profile:?}-editor-physical-source-alias"),
            tool: "run_command".to_string(),
            args: json!({
                "command": format!(
                    "python {editor} --source {logical_source} --output {source}"
                ),
                "inputs": [{ "path": source, "mountPath": logical_source }]
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = command_request_from_call(&context, &aliased_source)
            .expect_err("save-as must compare the physical source and destination identities");
        assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));

        let mut tampered_purpose = request.clone();
        tampered_purpose
            .managed_office_script
            .as_deref_mut()
            .unwrap()
            .purpose = crate::office::OfficeManagedScriptPurpose::Create;
        validate_frozen_command_trace_args(&tampered_purpose, &valid.args)
            .expect_err("restart trace must bind the hidden Editor purpose to the command");
        let mut tampered_destination = request.clone();
        tampered_destination
            .managed_office_script
            .as_deref_mut()
            .unwrap()
            .destination
            .logical_path = format!("other-{output}");
        validate_frozen_command_trace_args(&tampered_destination, &valid.args)
            .expect_err("restart trace must bind the hidden destination to --output");
    }
}

#[test]
fn exact_bundled_editor_receipt_freezes_the_script_as_a_host_reserved_input() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("outputs")).unwrap();
    let editor = "scripts/edit_existing.mjs";
    std::fs::create_dir_all(workspace.path().join("scripts")).unwrap();
    let editor_bytes = b"// materialized fixed editor\n";
    std::fs::write(workspace.path().join(editor), editor_bytes).unwrap();
    let source_bytes = b"PK\x03\x04presentation fixture";
    std::fs::write(workspace.path().join("source.pptx"), source_bytes).unwrap();
    let storage = Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
    let run_id = "run-presentation-editor-provenance";
    record_materialized_presentation_editor(
        &storage,
        run_id,
        APPLICATION_BUNDLED_SKILL_SOURCE_ID,
        editor,
    );
    let args = json!({
        "command": format!(
            "node {editor} --source source.pptx --output outputs/source-edited.pptx"
        ),
        "inputs": [{ "path": "source.pptx", "mountPath": "source.pptx" }]
    });
    let call = AgentToolCall {
        id: "tool-presentation-editor-provenance".to_string(),
        tool: "run_command".to_string(),
        args: args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-presentation-editor".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("presentation editor".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        })),
        test_binding(
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Node,
            &[("pptxgenjs", "4.0.1")],
        ),
    )
    .with_runtime_services(run_id.to_string(), Some(storage));

    let request = command_request_from_call(&context, &call).unwrap();
    assert_eq!(
        request
            .runtime_binding
            .as_deref()
            .map(|binding| binding.profile),
        Some(AgentCommandRuntimeProfile::Presentations)
    );
    assert_eq!(request.inputs.len(), 2);
    let script = request
        .inputs
        .iter()
        .find(|input| input.mount_path == crate::command::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
        .expect("the exact bundled Editor script is a Host-reserved frozen input");
    assert_eq!(
        script.source,
        AgentFileInputRef::Workspace {
            path: editor.to_string()
        }
    );
    assert_eq!(script.size_bytes, editor_bytes.len() as u64);
    assert_eq!(script.sha256, format!("{:x}", Sha256::digest(editor_bytes)));
    let source = request
        .inputs
        .iter()
        .find(|input| input.mount_path == "source.pptx")
        .expect("source deck remains independently frozen");
    assert_eq!(source.size_bytes, source_bytes.len() as u64);
    validate_frozen_command_trace_args(&request, &args).unwrap();

    let aliased_source_args = json!({
        "command": format!(
            "node {editor} --source logical-source.pptx --output source.pptx"
        ),
        "inputs": [{ "path": "source.pptx", "mountPath": "logical-source.pptx" }]
    });
    let aliased_source_call = AgentToolCall {
        id: "tool-presentation-editor-physical-source-alias".to_string(),
        tool: "run_command".to_string(),
        args: aliased_source_args,
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let error = command_request_from_call(&context, &aliased_source_call)
        .expect_err("Presentation save-as must reject a physical source/output alias");
    assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));

    let restored: AgentCommandRequest =
        serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
    validate_frozen_command_trace_args(&restored, &args)
        .expect("restart reconciliation ignores only the exact Host-hidden script binding");
    assert_eq!(restored.inputs, request.inputs);

    let mut tampered_source = request.clone();
    tampered_source
        .managed_office_script
        .as_deref_mut()
        .unwrap()
        .source_mount_path = Some("other-source.pptx".to_string());
    validate_frozen_command_trace_args(&tampered_source, &args)
        .expect_err("restart trace must bind the hidden source mount to --source");
    let mut tampered_destination = request.clone();
    tampered_destination
        .managed_office_script
        .as_deref_mut()
        .unwrap()
        .destination
        .logical_path = "outputs/other.pptx".to_string();
    validate_frozen_command_trace_args(&tampered_destination, &args)
        .expect_err("restart trace must bind the hidden destination to --output");

    let hidden = request
        .inputs
        .iter()
        .find(|input| input.mount_path == PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
        .unwrap()
        .clone();
    let mut duplicate_hidden = request.clone();
    duplicate_hidden.inputs.push(hidden.clone());
    validate_frozen_command_trace_args(&duplicate_hidden, &args)
        .expect_err("multiple hidden Editor identities must fail closed");

    let mut malformed_hidden = request.clone();
    malformed_hidden.inputs.push(crate::AgentFileInputBinding {
        mount_path: format!("{PRESENTATION_EDITOR_RESERVED_MOUNT_PREFIX}/unexpected.mjs"),
        ..hidden.clone()
    });
    validate_frozen_command_trace_args(&malformed_hidden, &args)
        .expect_err("unknown inputs beneath the Host-reserved prefix must fail closed");

    let mut external_hidden = request.clone();
    external_hidden
        .inputs
        .iter_mut()
        .find(|input| input.mount_path == PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
        .unwrap()
        .source = AgentFileInputRef::External {
        path: "/tmp/forged-editor.mjs".to_string(),
    };
    validate_frozen_command_trace_args(&external_hidden, &args)
        .expect_err("the hidden Editor script must remain workspace-owned");

    let mut wrong_shape = request.clone();
    wrong_shape.command = format!("node {editor} --output outputs/source-edited.pptx");
    let mut wrong_shape_args = args.clone();
    wrong_shape_args["command"] = json!(wrong_shape.command);
    validate_frozen_command_trace_args(&wrong_shape, &wrong_shape_args)
        .expect_err("the hidden identity cannot survive a command-shape downgrade");

    let syntax_args = json!({
        "command": format!("node --check {editor}")
    });
    let syntax_call = AgentToolCall {
        id: "tool-presentation-editor-syntax-check".to_string(),
        tool: "run_command".to_string(),
        args: syntax_args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let syntax_request = command_request_from_call(&context, &syntax_call)
        .expect("the exact Editor receipt must authorize its syntax-only check");
    assert_eq!(
        syntax_request
            .runtime_binding
            .as_deref()
            .map(|binding| (binding.profile, binding.kind)),
        Some((
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Node
        ))
    );
    assert!(
        syntax_request.inputs.is_empty(),
        "syntax-only checks must not receive the Host-hidden execution identity"
    );
    assert!(syntax_request.observe.is_none());
    validate_frozen_command_trace_args(&syntax_request, &syntax_args).unwrap();

    let unicode_source = "素材/南京 大学.pptx";
    std::fs::create_dir_all(workspace.path().join("素材")).unwrap();
    std::fs::write(
        workspace.path().join(unicode_source),
        b"PK\x03\x04unicode deck",
    )
    .unwrap();
    let quoted_args = json!({
        "command": format!(
            "node '{editor}' --source '南京 大学.pptx' --output 'outputs/南京 大学-编辑.pptx'"
        ),
        "inputs": [{ "path": unicode_source, "mountPath": "南京 大学.pptx" }]
    });
    let quoted_call = AgentToolCall {
        id: "tool-presentation-editor-quoted-unicode".to_string(),
        tool: "run_command".to_string(),
        args: quoted_args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let quoted_request = command_request_from_call(&context, &quoted_call)
        .expect("quoted spaces and Unicode paths must keep the exact Editor contract");
    assert!(quoted_request
        .inputs
        .iter()
        .any(|input| input.mount_path == "南京 大学.pptx"));
    assert!(quoted_request
        .inputs
        .iter()
        .any(|input| input.mount_path == PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH));
    assert_eq!(
        quoted_request
            .observe
            .as_ref()
            .and_then(|observe| observe.expected_outputs.first())
            .map(String::as_str),
        Some("outputs/南京 大学-编辑.pptx")
    );
    validate_frozen_command_trace_args(&quoted_request, &quoted_args).unwrap();

    for unsafe_output in ["/tmp/escaped.pptx", "../escaped.pptx"] {
        let unsafe_args = json!({
            "command": format!(
                "node {editor} --source source.pptx --output '{unsafe_output}'"
            ),
            "inputs": [{ "path": "source.pptx", "mountPath": "source.pptx" }]
        });
        let unsafe_call = AgentToolCall {
            id: format!("tool-presentation-editor-unsafe-output-{unsafe_output}"),
            tool: "run_command".to_string(),
            args: unsafe_args,
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = command_request_from_call(&context, &unsafe_call)
            .expect_err("Editor outputs must remain inside the effective write scope");
        assert_eq!(error.code(), Some("managedBuilder.outputOutsideWriteScope"));
    }

    let malformed_args = json!({
        "command": format!("node {editor} --output outputs/source-edited.pptx")
    });
    let malformed_call = AgentToolCall {
        id: "tool-presentation-editor-malformed".to_string(),
        tool: "run_command".to_string(),
        args: malformed_args,
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let error = command_request_from_call(&context, &malformed_call)
        .expect_err("an Editor receipt must never downgrade a malformed command to Builder");
    assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));
}

#[test]
fn third_party_editor_receipt_cannot_unlock_the_host_editor_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let editor = "scripts/edit_existing.mjs";
    std::fs::create_dir_all(workspace.path().join("scripts")).unwrap();
    std::fs::write(workspace.path().join(editor), "// spoofed editor\n").unwrap();
    std::fs::write(workspace.path().join("source.pptx"), b"PK\x03\x04fixture").unwrap();
    let storage = Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
    let run_id = "run-third-party-presentation-editor";
    record_materialized_presentation_editor(&storage, run_id, "workspace:workspace-1", editor);
    let base_args = json!({
        "command": format!(
            "node {editor} --source source.pptx --output outputs/source-edited.pptx"
        ),
        "inputs": [{ "path": "source.pptx", "mountPath": "source.pptx" }]
    });
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-third-party-editor".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("third-party editor".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        })),
        test_binding(
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Node,
            &[("pptxgenjs", "4.0.1")],
        ),
    )
    .with_runtime_services(run_id.to_string(), Some(storage));
    let call = AgentToolCall {
        id: "tool-third-party-editor".to_string(),
        tool: "run_command".to_string(),
        args: base_args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let ordinary_without_runtime = command_request_from_call(&context, &call)
        .expect("a third-party receipt may only leave this as an ordinary approved command");
    assert!(ordinary_without_runtime.runtime_binding.is_none());
    assert!(ordinary_without_runtime.managed_office_script.is_none());
    assert!(ordinary_without_runtime.observe.is_none());
    assert_eq!(
        ordinary_without_runtime.risk_level,
        Some(AgentCommandRiskLevel::Unknown)
    );
    assert!(ordinary_without_runtime
        .inputs
        .iter()
        .all(|input| input.mount_path != crate::command::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH));

    let mut explicit_args = base_args;
    explicit_args["runtimeProfile"] = json!("presentations");
    let explicit_call = AgentToolCall {
        id: "tool-third-party-editor-explicit-runtime".to_string(),
        tool: "run_command".to_string(),
        args: explicit_args,
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let ordinary = command_request_from_call(&context, &explicit_call)
        .expect("an explicit ordinary Office runtime remains a separate compatibility path");
    assert!(ordinary.runtime_binding.is_some());
    assert!(ordinary.managed_office_script.is_none());
    assert!(ordinary
        .inputs
        .iter()
        .all(|input| input.mount_path != crate::command::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH));

    let forged_args = json!({
        "command": format!(
            "node {editor} --source source.pptx --output outputs/source-edited.pptx"
        ),
        "runtimeProfile": "presentations",
        "inputs": [{
            "path": editor,
            "mountPath": crate::command::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH
        }]
    });
    let forged_call = AgentToolCall {
        id: "tool-forged-editor-reserved-input".to_string(),
        tool: "run_command".to_string(),
        args: forged_args,
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let forged = command_request_from_call(&context, &forged_call)
        .expect_err("model arguments cannot forge the Host-reserved Editor binding");
    assert_eq!(forged.code(), Some("agent.fileInput.invalidRequest"));

    for (case, mount_path) in [
        (
            "reserved-sibling",
            format!("{PRESENTATION_EDITOR_RESERVED_MOUNT_PREFIX}/loader.mjs"),
        ),
        (
            "reserved-child",
            format!("{PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH}/payload"),
        ),
    ] {
        let forged_args = json!({
            "command": format!(
                "node {editor} --source source.pptx --output outputs/source-edited.pptx"
            ),
            "runtimeProfile": "presentations",
            "inputs": [{ "path": editor, "mountPath": mount_path }]
        });
        let forged_call = AgentToolCall {
            id: format!("tool-forged-editor-{case}"),
            tool: "run_command".to_string(),
            args: forged_args,
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let forged = command_request_from_call(&context, &forged_call)
            .expect_err("the entire Presentation Editor namespace is Host-reserved");
        assert_eq!(forged.code(), Some("agent.fileInput.invalidRequest"));
    }
}

#[test]
fn current_run_presentation_builder_syntax_check_uses_frozen_runtime_without_observation() {
    let workspace = tempfile::tempdir().unwrap();
    let script = "scripts/build_deck.mjs";
    let script_path = workspace.path().join(script);
    std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
    std::fs::write(&script_path, "export const deck = true;\n").unwrap();
    let storage = Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
    let run_id = "run-presentation-syntax-check";
    record_materialized_builder(
        &storage,
        run_id,
        AgentCommandRuntimeProfile::Presentations,
        script,
    );
    let args = json!({"command": format!("node --check {script}")});
    let call = AgentToolCall {
        id: "tool-presentation-syntax-check".to_string(),
        tool: "run_command".to_string(),
        args: args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-presentation-syntax-check".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("presentation syntax check".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        }))
        .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
        test_binding(
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Node,
            &[("pptxgenjs", "4.0.1")],
        ),
    )
    .with_runtime_services(run_id.to_string(), Some(storage));

    let request = command_request_from_call(&context, &call).unwrap();
    let binding = request
        .runtime_binding
        .as_deref()
        .expect("current-Run materialization binds the managed runtime");
    assert_eq!(binding.profile, AgentCommandRuntimeProfile::Presentations);
    assert_eq!(binding.kind, AgentCommandRuntimeKind::Node);
    assert!(request.observe.is_none());
    assert!(request.inputs.is_empty());
    validate_frozen_command_trace_args(&request, &args).unwrap();

    let restored: AgentCommandRequest =
        serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
    validate_frozen_command_trace_args(&restored, &args).unwrap();
    assert_eq!(restored.runtime_binding, request.runtime_binding);
}

#[test]
fn ordinary_node_syntax_check_is_not_rebound_without_a_matching_current_run_receipt() {
    let workspace = tempfile::tempdir().unwrap();
    let script = "scripts/build_deck.mjs";
    let script_path = workspace.path().join(script);
    std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
    std::fs::write(&script_path, "export const deck = true;\n").unwrap();
    let storage = Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
    record_materialized_builder(
        &storage,
        "different-run",
        AgentCommandRuntimeProfile::Presentations,
        script,
    );
    let base = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("untrusted syntax check".to_string()),
            root_path: Some(workspace.path().to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            ..Default::default()
        },
    }));
    let context = with_profile_resolver(
        base,
        test_binding(
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Node,
            &[("pptxgenjs", "4.0.1")],
        ),
    )
    .with_runtime_services("current-run".to_string(), Some(storage));

    let ordinary_args = json!({"command": format!("node --check {script}")});
    let ordinary_call = AgentToolCall {
        id: "tool-ordinary-syntax-check".to_string(),
        tool: "run_command".to_string(),
        args: ordinary_args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let ordinary = command_request_from_call(&context, &ordinary_call).unwrap();
    assert!(ordinary.runtime_binding.is_none());
    assert!(ordinary.observe.is_none());
    validate_frozen_command_trace_args(&ordinary, &ordinary_args).unwrap();

    let explicit_args = json!({
        "command": format!("node --check {script}"),
        "runtimeProfile": "presentations"
    });
    let explicit_call = AgentToolCall {
        id: "tool-explicit-managed-syntax-check".to_string(),
        tool: "run_command".to_string(),
        args: explicit_args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let explicit = command_request_from_call(&context, &explicit_call).unwrap();
    assert_eq!(
        explicit
            .runtime_binding
            .as_deref()
            .map(|binding| binding.profile),
        Some(AgentCommandRuntimeProfile::Presentations)
    );
    assert!(explicit.observe.is_none());
    validate_frozen_command_trace_args(&explicit, &explicit_args).unwrap();
}

#[test]
fn activated_pdf_skill_does_not_intercept_a_provenance_bound_office_builder() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("outputs")).unwrap();
    let script = "scripts/build.py";
    let script_path = workspace.path().join(script);
    std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
    std::fs::write(&script_path, "# managed builder\n").unwrap();
    let storage = Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
    let run_id = "run-pdf-and-documents";
    record_materialized_builder(
        &storage,
        run_id,
        AgentCommandRuntimeProfile::Documents,
        script,
    );
    let command = "python scripts/build.py --output outputs/report.docx";
    let call = AgentToolCall {
        id: "tool-pdf-and-documents".to_string(),
        tool: "run_command".to_string(),
        args: json!({ "command": command }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-pdf-and-documents".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("PDF and Documents".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        }))
        .with_skill_resources(Some(pdf_and_documents_skill_session())),
        test_binding(
            AgentCommandRuntimeProfile::Documents,
            AgentCommandRuntimeKind::Python,
            &[("python-docx", "1.2.0")],
        ),
    )
    .with_runtime_services(run_id.to_string(), Some(storage));

    let request = command_request_from_call(&context, &call).unwrap();
    assert_eq!(request.command, command);
    assert_eq!(
        request
            .runtime_binding
            .as_deref()
            .map(|binding| binding.profile),
        Some(AgentCommandRuntimeProfile::Documents)
    );
    assert_eq!(
        request
            .observe
            .as_ref()
            .map(|observe| observe.kinds.as_slice()),
        Some([AgentCommandArtifactObservationKind::Office].as_slice())
    );
}

#[test]
fn ordinary_saved_scripts_are_not_silently_rebound_without_backend_provenance() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("ordinary.py"), "print('ordinary')\n").unwrap();
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("ordinary-script".to_string()),
                root_path: Some(workspace.path().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        })),
        test_binding(
            AgentCommandRuntimeProfile::Documents,
            AgentCommandRuntimeKind::Python,
            &[("python-docx", "1.2.0")],
        ),
    );
    let call = AgentToolCall {
        id: "ordinary-script".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "python ordinary.py --output report.docx"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };

    let request = command_request_from_call(&context, &call).unwrap();
    assert!(request.runtime_binding.is_none());
    assert!(request.observe.is_none());
}

#[test]
fn backend_rejects_conflicting_builder_profile_and_workspace_escape() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("builder.py"), "# managed builder\n").unwrap();
    let storage = Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
    let run_id = "run-builder-scope";
    record_materialized_builder(
        &storage,
        run_id,
        AgentCommandRuntimeProfile::Documents,
        "builder.py",
    );
    let context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("managed-builder".to_string()),
                root_path: Some(workspace.path().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
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
    .with_runtime_services(run_id.to_string(), Some(Arc::clone(&storage)));
    let conflicting = AgentToolCall {
        id: "tool-conflicting-profile".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "python builder.py --output report.docx",
            "runtimeProfile": "spreadsheets"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let error = command_request_from_call(&context, &conflicting).unwrap_err();
    assert_eq!(error.code(), Some("managedBuilder.profileMismatch"));

    let escaping = AgentToolCall {
        id: "tool-escaping-output".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": "python builder.py --output ../outside.docx"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let error = command_request_from_call(&context, &escaping).unwrap_err();
    assert_eq!(error.code(), Some("managedBuilder.outputOutsideWriteScope"));

    let full_access_context = with_profile_resolver(
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("managed-builder".to_string()),
                root_path: Some(workspace.path().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::All,
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
    let external_output = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join("mycopilot-managed-builder.docx");
    let external_output = external_output.to_string_lossy().into_owned();
    let external = AgentToolCall {
        id: "tool-external-output".to_string(),
        tool: "run_command".to_string(),
        args: json!({
            "command": format!("python builder.py --output {external_output}")
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let request = command_request_from_call(&full_access_context, &external).unwrap();
    assert_eq!(
        request
            .observe
            .as_ref()
            .unwrap()
            .expected_outputs
            .as_slice(),
        [external_output]
    );
}
