use super::*;

#[test]
fn presentation_whole_slide_operations_compile_to_bounded_officecli_arguments() {
    let fixture = Fixture::new(basic_script());
    let cases = [
        (
            OfficeOperation::Add,
            OfficeOperationParameters::Add {
                parent: "/".to_string(),
                element_type: "slide".to_string(),
                copy_from: None,
                position: None,
                properties: [
                    ("title".to_string(), serde_json::json!("Appendix")),
                    ("background".to_string(), serde_json::json!("F8FAFC")),
                ]
                .into_iter()
                .collect(),
                force: false,
            },
            vec![
                "/",
                "--type",
                "slide",
                "--prop",
                "background=F8FAFC",
                "--prop",
                "title=Appendix",
                "--json",
            ],
        ),
        (
            OfficeOperation::Remove,
            OfficeOperationParameters::Remove {
                target: "/slide[7]".to_string(),
                shift: None,
                properties: BTreeMap::new(),
            },
            vec!["/slide[7]", "--json"],
        ),
        (
            OfficeOperation::Move,
            OfficeOperationParameters::Move {
                target: "/slide[7]".to_string(),
                new_parent: None,
                position: Some(OfficeElementPosition::Index { index: 2 }),
                properties: BTreeMap::new(),
            },
            vec!["/slide[7]", "--index", "2", "--json"],
        ),
    ];

    for (operation, parameters, expected) in cases {
        let mut request = fixture.request(operation);
        request.document_kind = OfficeDocumentKind::Presentation;
        request.document_path = Some("source.pptx".to_string());
        request.parameters = parameters;
        assert_eq!(
            compile_office_arguments(&request).unwrap(),
            expected.into_iter().map(str::to_string).collect::<Vec<_>>()
        );
    }
}

#[test]
fn managed_script_validation_cancellation_and_destination_conflict_never_publish() {
    let script = r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "validate" ]; then printf '{"success":true}\n'; exit 0; fi
exit 64
"#;

    // Invalid bytes fail the Host package gate before the provider validation command.
    let invalid = Fixture::new(script);
    let invalid_context = workspace_context(invalid.workspace.path());
    let invalid_binding = prepare_managed_script_binding(
        &invalid_context,
        OfficeDocumentKind::Document,
        OfficeManagedScriptPurpose::Create,
        "__mycopilot/managed-office-script/documents/builder.py".to_string(),
        None,
        "invalid.docx",
    )
    .unwrap();
    let mut invalid_staging =
        prepare_managed_script_staging(&invalid_context, &invalid_binding).unwrap();
    fs::write(invalid_staging.candidate_path(), b"not an OOXML package").unwrap();
    let invalid_result = invalid
        .engine
        .commit_managed_script_output(
            &invalid_context,
            &mut invalid_staging,
            None,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(
        invalid_result.error_code.as_deref(),
        Some("office.invalid_output")
    );
    assert!(!invalid.workspace.path().join("invalid.docx").exists());

    // A cancellation arriving after strict validation but before rename cannot publish.
    let cancelled = Fixture::new(script);
    let cancelled_context = workspace_context(cancelled.workspace.path());
    let cancelled_binding = prepare_managed_script_binding(
        &cancelled_context,
        OfficeDocumentKind::Document,
        OfficeManagedScriptPurpose::Create,
        "__mycopilot/managed-office-script/documents/builder.py".to_string(),
        None,
        "cancelled.docx",
    )
    .unwrap();
    let mut cancelled_staging =
        prepare_managed_script_staging(&cancelled_context, &cancelled_binding).unwrap();
    write_docx(cancelled_staging.candidate_path(), "candidate");
    let cancel_flag = Arc::new(AtomicBool::new(false));
    install_commit_test_hook(
        PathBuf::from(&cancelled_binding.destination.normalized_path),
        CommitTestPhase::BeforeCancellationCheck,
        cancel_flag.clone(),
    );
    let cancelled_result = cancelled
        .engine
        .commit_managed_script_output(
            &cancelled_context,
            &mut cancelled_staging,
            None,
            AgentCancellationToken::new(),
            Some(cancel_flag),
        )
        .unwrap();
    assert!(cancelled_result.cancelled);
    assert_eq!(
        cancelled_result.error_code.as_deref(),
        Some("office.cancelled")
    );
    assert!(!cancelled.workspace.path().join("cancelled.docx").exists());

    // A destination created after approval wins; the private candidate is never allowed to
    // overwrite it, and the conflict is reported as a frozen-precondition failure.
    let conflicted = Fixture::new(script);
    let conflicted_context = workspace_context(conflicted.workspace.path());
    let conflicted_binding = prepare_managed_script_binding(
        &conflicted_context,
        OfficeDocumentKind::Document,
        OfficeManagedScriptPurpose::Create,
        "__mycopilot/managed-office-script/documents/builder.py".to_string(),
        None,
        "conflicted.docx",
    )
    .unwrap();
    let mut conflicted_staging =
        prepare_managed_script_staging(&conflicted_context, &conflicted_binding).unwrap();
    write_docx(conflicted_staging.candidate_path(), "candidate");
    let target = conflicted.workspace.path().join("conflicted.docx");
    write_docx(&target, "concurrent destination");
    let concurrent_bytes = fs::read(&target).unwrap();
    let conflicted_result = conflicted
        .engine
        .commit_managed_script_output(
            &conflicted_context,
            &mut conflicted_staging,
            None,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(
        conflicted_result.error_code.as_deref(),
        Some("office.precondition_failed")
    );
    assert_eq!(fs::read(target).unwrap(), concurrent_bytes);
}

#[test]
fn document_managed_script_keeps_officecli_schema_diagnostics() {
    let fixture = Fixture::new(
        r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "validate" ]; then
  printf '{"success":false,"issues":[{"type":"schema","description":"Duplicate child element.","path":"/w:tbl/w:tblPr/w:tblLayout[2]","part":"word/document.xml"}]}\n'
  exit 7
fi
exit 64
"#,
    );
    let context = workspace_context(fixture.workspace.path());
    let binding = prepare_managed_script_binding(
        &context,
        OfficeDocumentKind::Document,
        OfficeManagedScriptPurpose::Create,
        "__mycopilot/managed-office-script/documents/builder.py".to_string(),
        None,
        "rejected.docx",
    )
    .unwrap();
    let mut staging = prepare_managed_script_staging(&context, &binding).unwrap();
    write_docx(staging.candidate_path(), "candidate");

    let result = fixture
        .engine
        .commit_managed_script_output(
            &context,
            &mut staging,
            None,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert_eq!(result.error_code.as_deref(), Some("office.nonzero_exit"));
    for diagnostic in [
        "schema",
        "Duplicate child element.",
        "/w:tbl/w:tblPr/w:tblLayout[2]",
        "word/document.xml",
    ] {
        assert!(result.stdout.contains(diagnostic));
    }
    assert!(!fixture.workspace.path().join("rejected.docx").exists());
}

#[test]
fn spreadsheet_managed_script_uses_only_frozen_python_reopen_gate() {
    let office_evidence = tempfile::tempdir().unwrap();
    let office_marker_path = office_evidence.path().join("officecli-called");
    let fixture = Fixture::new(&format!(
        r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "validate" ]; then printf called > '{}'; exit 91; fi
exit 64
"#,
        office_marker_path.display()
    ));
    let python_dir = tempfile::tempdir().unwrap();
    let python_marker = python_dir.path().join("python-args.txt");
    let python = python_dir.path().join("python3");
    write_executable(
        &python,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            python_marker.display()
        ),
    );
    let invocation = fake_python_invocation(python);
    let context = workspace_context(fixture.workspace.path());
    let binding = prepare_managed_script_binding(
        &context,
        OfficeDocumentKind::Spreadsheet,
        OfficeManagedScriptPurpose::Create,
        "__mycopilot/managed-office-script/spreadsheets/builder.py".to_string(),
        None,
        "created.xlsx",
    )
    .unwrap();
    let mut staging = prepare_managed_script_staging(&context, &binding).unwrap();
    write_xlsx_package(staging.candidate_path());

    let result = fixture
        .engine
        .commit_managed_script_output(
            &context,
            &mut staging,
            Some(&invocation),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert_eq!(result.exit_code, Some(0), "{:?}", result.error);
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert!(fixture.workspace.path().join("created.xlsx").is_file());
    assert!(
        !office_marker_path.exists(),
        "OfficeCLI validate must not run"
    );
    let python_arguments = fs::read_to_string(python_marker).unwrap();
    assert!(python_arguments.contains("openpyxl"));
    assert!(python_arguments.contains("created.xlsx"));
}

#[test]
fn spreadsheet_openpyxl_reopen_failure_never_publishes() {
    let fixture = Fixture::new(
        "if [ \"$1\" = \"--version\" ]; then printf 'OfficeCLI 1.2.3\\n'; exit 0; fi\nexit 88\n",
    );
    let python_dir = tempfile::tempdir().unwrap();
    let python = python_dir.path().join("python3");
    write_executable(
        &python,
        "#!/bin/sh\nprintf 'openpyxl rejected workbook' >&2\nexit 7\n",
    );
    let invocation = fake_python_invocation(python);
    let context = workspace_context(fixture.workspace.path());
    let binding = prepare_managed_script_binding(
        &context,
        OfficeDocumentKind::Spreadsheet,
        OfficeManagedScriptPurpose::Create,
        "__mycopilot/managed-office-script/spreadsheets/builder.py".to_string(),
        None,
        "rejected.xlsx",
    )
    .unwrap();
    let mut staging = prepare_managed_script_staging(&context, &binding).unwrap();
    write_xlsx_package(staging.candidate_path());

    let result = fixture
        .engine
        .commit_managed_script_output(
            &context,
            &mut staging,
            Some(&invocation),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert_eq!(result.error_code.as_deref(), Some("office.invalid_output"));
    assert!(result.stderr.contains("openpyxl rejected workbook"));
    assert!(!fixture.workspace.path().join("rejected.xlsx").exists());
}

#[test]
fn spreadsheet_publication_does_not_require_an_available_officecli() {
    let workspace = tempfile::tempdir().unwrap();
    let context = workspace_context(workspace.path());
    let binding = prepare_managed_script_binding(
        &context,
        OfficeDocumentKind::Spreadsheet,
        OfficeManagedScriptPurpose::Create,
        "__mycopilot/managed-office-script/spreadsheets/builder.py".to_string(),
        None,
        "created.xlsx",
    )
    .unwrap();
    let mut staging = prepare_managed_script_staging(&context, &binding).unwrap();
    write_xlsx_package(staging.candidate_path());
    let python_dir = tempfile::tempdir().unwrap();
    let python = python_dir.path().join("python3");
    write_executable(&python, "#!/bin/sh\nexit 0\n");
    let invocation = fake_python_invocation(python);
    let unavailable = UnavailableOfficeEngine::new(OfficeEngineError::new(
        OfficeEngineErrorCode::Unavailable,
        OfficeEngineRecovery::InstallComponent,
        "OfficeCLI is absent",
    ));

    let result = unavailable
        .commit_managed_script_output(
            &context,
            &mut staging,
            Some(&invocation),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(result.error_code.is_none());
    assert!(workspace.path().join("created.xlsx").is_file());
}

#[test]
#[ignore = "requires MYCOPILOT_OFFICE_TEST_PYTHON with openpyxl"]
fn real_openpyxl_workbook_passes_host_reopen_gate_without_officecli_validate() {
    let python = std::env::var_os("MYCOPILOT_OFFICE_TEST_PYTHON")
        .map(PathBuf::from)
        .expect("set MYCOPILOT_OFFICE_TEST_PYTHON to managed Python with openpyxl");
    let office_evidence = tempfile::tempdir().unwrap();
    let office_marker = office_evidence.path().join("officecli-called");
    let fixture = Fixture::new(&format!(
        r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "validate" ]; then printf called > '{}'; exit 91; fi
exit 64
"#,
        office_marker.display()
    ));
    let context = workspace_context(fixture.workspace.path());
    let binding = prepare_managed_script_binding(
        &context,
        OfficeDocumentKind::Spreadsheet,
        OfficeManagedScriptPurpose::Create,
        "__mycopilot/managed-office-script/spreadsheets/builder.py".to_string(),
        None,
        "real.xlsx",
    )
    .unwrap();
    let mut staging = prepare_managed_script_staging(&context, &binding).unwrap();
    let created = std::process::Command::new(&python)
        .args([
            "-I",
            "-B",
            "-c",
            "from openpyxl import Workbook; import sys; w=Workbook(); w.active['A1']='ok'; w.save(sys.argv[1])",
        ])
        .arg(staging.candidate_path())
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let invocation = ArtifactRuntimeInvocation::new(
        "test-bundle".to_string(),
        "test-revision".to_string(),
        "test-fingerprint".to_string(),
        ArtifactRuntimeKind::Python,
        "3.12".to_string(),
        python.clone(),
        vec![OsString::from("-I"), OsString::from("-B")],
        BTreeMap::new(),
    );

    let result = fixture
        .engine
        .commit_managed_script_output(
            &context,
            &mut staging,
            Some(&invocation),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert_eq!(result.exit_code, Some(0), "{:?}", result.error);
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert!(!office_marker.exists(), "OfficeCLI validate must not run");
    let verified = std::process::Command::new(python)
        .args([
            "-I",
            "-B",
            "-c",
            "from openpyxl import load_workbook; import sys; w=load_workbook(sys.argv[1]); assert w.active['A1'].value == 'ok'; w.close()",
        ])
        .arg(fixture.workspace.path().join("real.xlsx"))
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
}

#[test]
fn host_managed_help_topics_never_compile_to_officecli_verbs() {
    let fixture = Fixture::new(basic_script());
    for topic in [
        OfficeHelpVerb::Status,
        OfficeHelpVerb::Help,
        OfficeHelpVerb::Create,
        OfficeHelpVerb::View,
        OfficeHelpVerb::Validate,
        OfficeHelpVerb::Move,
        OfficeHelpVerb::Swap,
    ] {
        let mut request = fixture.request(OfficeOperation::Help);
        request.document_path = None;
        request.parameters = OfficeOperationParameters::Help {
            verb: Some(topic),
            element: Some("document".to_string()),
        };

        let error = compile_office_arguments(&request)
            .expect_err("Host-managed help must fail closed at the provider boundary");
        assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
        assert!(error.message().contains("Host-managed"));
        assert!(error.message().contains(topic.stable_name()));
        assert!(!error.message().contains("unknown element"));
    }

    let mut provider_help = fixture.request(OfficeOperation::Help);
    provider_help.document_path = None;
    provider_help.parameters = OfficeOperationParameters::Help {
        verb: Some(OfficeHelpVerb::Add),
        element: Some("document".to_string()),
    };
    assert_eq!(
        compile_office_arguments(&provider_help).unwrap(),
        vec!["docx", "add", "document", "--json"]
    );
}

#[test]
fn typed_operation_parameters_serialize_with_camel_case_fields() {
    let view = OfficeOperationParameters::View {
        mode: OfficeViewMode::Screenshot,
        start: None,
        end: None,
        max_lines: Some(120),
        issue_type: Some("format".to_string()),
        limit: None,
        columns: Vec::new(),
        pages: Vec::new(),
        range: None,
        viewport: None,
        grid: Some(OfficeGridLayout::Columns { columns: 3 }),
        render_mode: Some(OfficeViewRenderMode::Html),
        page_count: true,
    };
    let serialized = serde_json::to_value(&view).unwrap();
    assert_eq!(serialized["maxLines"], 120);
    assert_eq!(serialized["issueType"], "format");
    assert_eq!(
        serialized["grid"],
        serde_json::json!({ "mode": "columns", "columns": 3 })
    );
    assert_eq!(serialized["renderMode"], "html");
    assert_eq!(serialized["pageCount"], true);
    for legacy_name in [
        "max_lines",
        "issue_type",
        "grid_columns",
        "render_mode",
        "page_count",
    ] {
        assert!(serialized.get(legacy_name).is_none());
    }
    assert_eq!(
        serde_json::from_value::<OfficeOperationParameters>(serialized).unwrap(),
        view
    );

    let add = OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "paragraph".to_string(),
        copy_from: Some("/body/p[1]".to_string()),
        position: None,
        properties: OfficePropertyMap::new(),
        force: false,
    };
    let serialized = serde_json::to_value(&add).unwrap();
    assert_eq!(serialized["elementType"], "paragraph");
    assert_eq!(serialized["copyFrom"], "/body/p[1]");
    assert!(serialized.get("element_type").is_none());
    assert!(serialized.get("copy_from").is_none());

    let swap = OfficeOperationParameters::Swap {
        first_target: "/body/p[1]".to_string(),
        second_target: "/body/p[2]".to_string(),
    };
    let serialized = serde_json::to_value(&swap).unwrap();
    assert_eq!(serialized["firstTarget"], "/body/p[1]");
    assert_eq!(serialized["secondTarget"], "/body/p[2]");
}

#[test]
fn typed_operation_validation_reports_domain_errors_before_spawn() {
    let fixture = Fixture::new(basic_script());

    let mut query = fixture.request(OfficeOperation::Query);
    query.parameters = OfficeOperationParameters::Query {
        selector: "/slide".to_string(),
        contains: None,
        compact: false,
        fields: Vec::new(),
    };
    let error = compile_office_arguments(&query).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
    assert!(error.message().contains("use get"));

    let mut mismatched = fixture.request(OfficeOperation::Validate);
    mismatched.parameters = default_parameters(OfficeOperation::Get);
    let error = compile_office_arguments(&mismatched).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    let mut nested_property = fixture.request(OfficeOperation::Set);
    nested_property.parameters = OfficeOperationParameters::Set {
        target: "/body".to_string(),
        properties: [("text".to_string(), serde_json::json!({"nested": true}))]
            .into_iter()
            .collect(),
        replacement: None,
        force: false,
    };
    let error = compile_office_arguments(&nested_property).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    let mut spreadsheet_view = fixture.request(OfficeOperation::View);
    spreadsheet_view.document_kind = OfficeDocumentKind::Spreadsheet;
    spreadsheet_view.document_path = Some("budget.xlsx".to_string());
    let OfficeOperationParameters::View { mode, grid, .. } = &mut spreadsheet_view.parameters
    else {
        panic!("fixture must build a typed view request");
    };
    *mode = OfficeViewMode::Screenshot;
    *grid = Some(OfficeGridLayout::Columns { columns: 2 });
    let error = compile_office_arguments(&spreadsheet_view).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    let mut spreadsheet_query = fixture.request(OfficeOperation::Query);
    spreadsheet_query.document_kind = OfficeDocumentKind::Spreadsheet;
    spreadsheet_query.document_path = Some("budget.xlsx".to_string());
    let OfficeOperationParameters::Query { compact, .. } = &mut spreadsheet_query.parameters else {
        panic!("fixture must build a typed query request");
    };
    *compact = true;
    let error = compile_office_arguments(&spreadsheet_query).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    let mut issues = fixture.request(OfficeOperation::View);
    issues.parameters = OfficeOperationParameters::View {
        mode: OfficeViewMode::Issues,
        start: None,
        end: None,
        max_lines: None,
        issue_type: Some("formula_eval_error".to_string()),
        limit: None,
        columns: Vec::new(),
        pages: Vec::new(),
        range: None,
        viewport: None,
        grid: None,
        render_mode: None,
        page_count: false,
    };
    let error = compile_office_arguments(&issues).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    let mut zero_start = fixture.request(OfficeOperation::View);
    let OfficeOperationParameters::View { start, .. } = &mut zero_start.parameters else {
        panic!("fixture must build a typed view request");
    };
    *start = Some(0);
    let error = compile_office_arguments(&zero_start).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
}

#[test]
fn workspace_paths_and_file_properties_are_contained() {
    let fixture = Fixture::new(basic_script());
    let mut request = fixture.request(OfficeOperation::Validate);
    request.document_path = Some("../secret.docx".to_string());
    let error = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);

    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let mut request = fixture.request(OfficeOperation::Add);
    request.parameters = OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "picture".to_string(),
        copy_from: None,
        position: None,
        properties: [(
            "src".to_string(),
            serde_json::json!({ "resourcePath": "/etc/passwd" }),
        )]
        .into_iter()
        .collect(),
        force: false,
    };
    let error = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
}

#[test]
fn agent_file_inputs_are_frozen_revalidated_and_exactly_referenced() {
    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "replacement");
    let script = format!(
        "#!/bin/sh\nresource=''\nfor argument in \"$@\"; do case \"$argument\" in src=*) resource=${{argument#src=}} ;; esac; done\n/bin/cat \"$resource\"\n/bin/cp '{}' \"$2\"\n",
        replacement.path().display()
    );
    let fixture = Fixture::new(&script);
    write_docx(&fixture.workspace.path().join("sample.docx"), "original");
    fs::write(fixture.workspace.path().join("hero.png"), b"approved-image").unwrap();

    let mount_path = "office/image-input-1.png".to_string();
    let placeholder = office_agent_input_placeholder(&mount_path);
    let mut request = fixture.request(OfficeOperation::Add);
    request.inputs = vec![AgentFileInputSpec {
        mount_path: mount_path.clone(),
        source: AgentFileInputRef::Workspace {
            path: "hero.png".to_string(),
        },
    }];
    request.parameters = OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "picture".to_string(),
        copy_from: None,
        position: None,
        properties: [(
            "src".to_string(),
            serde_json::json!({ "resourcePath": placeholder }),
        )]
        .into_iter()
        .collect(),
        force: false,
    };
    let context = workspace_context(fixture.workspace.path());
    let prepared = fixture.engine.prepare(&context, &request).unwrap();
    assert_eq!(prepared.input_bindings.len(), 1);
    assert!(!prepared
        .paths
        .iter()
        .any(|path| matches!(path.slot, OfficePathSlot::Resource { .. })));

    let mut mismatched = prepared.clone();
    mismatched.input_bindings[0].mount_path = "office/other.png".to_string();
    let error = fixture
        .engine
        .execute_prepared(&context, &mismatched, AgentCancellationToken::new(), None)
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);

    fs::write(fixture.workspace.path().join("hero.png"), b"changed").unwrap();
    let error = fixture
        .engine
        .execute_prepared(&context, &prepared, AgentCancellationToken::new(), None)
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);

    fs::write(fixture.workspace.path().join("hero.png"), b"approved-image").unwrap();
    let result = fixture
        .engine
        .execute_prepared(&context, &prepared, AgentCancellationToken::new(), None)
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "{:?}", result.error);
    assert!(result.stdout.contains("approved-image"));

    let mut unreferenced = request.clone();
    unreferenced.parameters = OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "picture".to_string(),
        copy_from: None,
        position: None,
        properties: BTreeMap::new(),
        force: false,
    };
    assert_eq!(
        fixture
            .engine
            .prepare(&context, &unreferenced)
            .unwrap_err()
            .code(),
        OfficeEngineErrorCode::InvalidRequest
    );

    let mut duplicate = request.clone();
    duplicate.parameters = OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "picture".to_string(),
        copy_from: None,
        position: None,
        properties: [
            (
                "src".to_string(),
                serde_json::json!({ "resourcePath": office_agent_input_placeholder(&mount_path) }),
            ),
            (
                "image".to_string(),
                serde_json::json!({ "resourcePath": office_agent_input_placeholder(&mount_path) }),
            ),
        ]
        .into_iter()
        .collect(),
        force: false,
    };
    assert_eq!(
        fixture
            .engine
            .prepare(&context, &duplicate)
            .unwrap_err()
            .code(),
        OfficeEngineErrorCode::InvalidRequest
    );
}

#[test]
fn path_bearing_properties_are_frozen_and_snapshotted() {
    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "replacement");
    let signal = tempfile::NamedTempFile::new().unwrap();
    let signal_path = signal.path().to_path_buf();
    drop(signal);
    let script = format!(
        "#!/bin/sh\nresource=''\nfor argument in \"$@\"; do case \"$argument\" in src=*) resource=${{argument#src=}} ;; esac; done\nprintf ready > '{}'\n/bin/sleep 0.15\n/bin/cat \"$resource\"\n/bin/cp '{}' \"$2\"\n",
        signal_path.display(),
        replacement.path().display()
    );
    let fixture = Fixture::new(&script);
    let document = fixture.workspace.path().join("sample.docx");
    let resource = fixture.workspace.path().join("asset.bin");
    write_docx(&document, "original");
    fs::write(&resource, b"approved-resource").unwrap();
    let mut request = fixture.request(OfficeOperation::Add);
    request.parameters = OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "picture".to_string(),
        copy_from: None,
        position: None,
        properties: [(
            "src".to_string(),
            serde_json::json!({ "resourcePath": "asset.bin" }),
        )]
        .into_iter()
        .collect(),
        force: false,
    };
    let prepared = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &request)
        .unwrap();
    let resource_plan = prepared_path(&prepared, &OfficePathSlot::Resource { index: 0 });
    assert_eq!(resource_plan.logical_path, "asset.bin");
    assert_eq!(resource_plan.purpose, OfficePathPurpose::ReadSource);

    let workspace = fixture.workspace.path().to_path_buf();
    let engine = fixture.engine.clone();
    let execution = thread::spawn(move || {
        engine
            .execute_prepared(
                &workspace_context(&workspace),
                &prepared,
                AgentCancellationToken::new(),
                None,
            )
            .unwrap()
    });
    // A full workspace test run can briefly saturate process creation on CI. Keep the
    // synchronization bounded, but do not mistake scheduler delay for a provider failure.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !signal_path.exists() && std::time::Instant::now() < deadline {
        thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        signal_path.exists(),
        "the fixture provider did not reach its resource-read boundary"
    );
    fs::write(&resource, b"changed-after-provider-start").unwrap();
    let result = execution.join().unwrap();
    assert!(result.stdout.contains("approved-resource"));
    assert!(!result.stdout.contains("changed-after-provider-start"));
    assert_eq!(
        result.error_code.as_deref(),
        Some("office.precondition_failed")
    );

    for alias in ["path", "fallback", "poster", "preview", "imagefill"] {
        let mut request = fixture.request(OfficeOperation::Add);
        request.parameters = OfficeOperationParameters::Add {
            parent: "/body".to_string(),
            element_type: "picture".to_string(),
            copy_from: None,
            position: None,
            properties: [(
                alias.to_string(),
                serde_json::json!({ "resourcePath": "/etc/passwd" }),
            )]
            .into_iter()
            .collect(),
            force: false,
        };
        let error = fixture
            .engine
            .prepare(&workspace_context(fixture.workspace.path()), &request)
            .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    }
}

#[test]
fn composite_resources_and_diagram_rendering_are_host_controlled() {
    let fixture = Fixture::new(basic_script());
    write_docx(&fixture.workspace.path().join("sample.docx"), "original");
    fs::write(fixture.workspace.path().join("background.png"), b"png").unwrap();

    let mut background = fixture.request(OfficeOperation::Set);
    background.parameters = OfficeOperationParameters::Set {
        target: "/body".to_string(),
        properties: [(
            "background".to_string(),
            serde_json::json!({ "resourcePath": "background.png" }),
        )]
        .into_iter()
        .collect(),
        replacement: None,
        force: false,
    };
    let prepared = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &background)
        .unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Resource { index: 0 }).logical_path,
        "background.png"
    );

    for property in ["background", "imagefill"] {
        let mut request = fixture.request(OfficeOperation::Set);
        request.parameters = OfficeOperationParameters::Set {
            target: "/body".to_string(),
            properties: [(
                property.to_string(),
                serde_json::json!({ "resourcePath": "/etc/passwd" }),
            )]
            .into_iter()
            .collect(),
            replacement: None,
            force: false,
        };
        let error = fixture
            .engine
            .prepare(&workspace_context(fixture.workspace.path()), &request)
            .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    }

    let mut data = fixture.request(OfficeOperation::Add);
    data.parameters = OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "table".to_string(),
        copy_from: None,
        position: None,
        properties: [("data".to_string(), serde_json::json!("/etc/passwd"))]
            .into_iter()
            .collect(),
        force: false,
    };
    let error = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &data)
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::UnsafeOperation);

    for render in ["auto", "image"] {
        let mut diagram = fixture.request(OfficeOperation::Add);
        diagram.parameters = OfficeOperationParameters::Add {
            parent: "/body".to_string(),
            element_type: "diagram".to_string(),
            copy_from: None,
            position: None,
            properties: [("render".to_string(), serde_json::json!(render))]
                .into_iter()
                .collect(),
            force: false,
        };
        let error = fixture
            .engine
            .prepare(&workspace_context(fixture.workspace.path()), &diagram)
            .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
    }

    let mut native_diagram = fixture.request(OfficeOperation::Add);
    native_diagram.parameters = OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "diagram".to_string(),
        copy_from: None,
        position: None,
        properties: std::collections::BTreeMap::new(),
        force: false,
    };
    let prepared = fixture
        .engine
        .prepare(
            &workspace_context(fixture.workspace.path()),
            &native_diagram,
        )
        .unwrap();
    assert!(prepared
        .argv
        .windows(2)
        .any(|pair| pair == ["--prop", "render=native"]));

    for (name, value) in [
        ("poster", serde_json::json!(true)),
        ("path", serde_json::json!("line")),
    ] {
        let mut ambiguous_but_non_file = fixture.request(OfficeOperation::Add);
        ambiguous_but_non_file.parameters = OfficeOperationParameters::Add {
            parent: "/body".to_string(),
            element_type: "shape".to_string(),
            copy_from: None,
            position: None,
            properties: [(name.to_string(), value)].into_iter().collect(),
            force: false,
        };
        fixture
            .engine
            .prepare(
                &workspace_context(fixture.workspace.path()),
                &ambiguous_but_non_file,
            )
            .unwrap();
    }
}
