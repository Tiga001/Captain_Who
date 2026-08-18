use super::*;

#[test]
#[ignore = "requires MYCOPILOT_OFFICECLI_PATH to point to a managed OfficeCLI component"]
fn real_officecli_creates_and_validates_all_supported_formats() {
    let executable = std::env::var_os("MYCOPILOT_OFFICECLI_PATH")
        .expect("MYCOPILOT_OFFICECLI_PATH is required for the real provider smoke test");
    let workspace = tempfile::tempdir().unwrap();
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(executable)
            .with_workspace_root(workspace.path()),
    )
    .unwrap();
    let status = engine.status(AgentCancellationToken::new());
    assert_eq!(status.availability, OfficeEngineAvailability::Available);

    for (document_kind, path) in [
        (OfficeDocumentKind::Document, "smoke.docx"),
        (OfficeDocumentKind::Spreadsheet, "smoke.xlsx"),
        (OfficeDocumentKind::Presentation, "smoke.pptx"),
    ] {
        let create = engine
            .execute(
                &workspace_context(workspace.path()),
                &OfficeExecutionRequest {
                    document_kind,
                    operation: OfficeOperation::Create,
                    document_path: Some(path.to_string()),
                    parameters: OfficeOperationParameters::Create {
                        locale: None,
                        minimal: false,
                        overwrite: false,
                    },
                    output_path: None,
                    destination_path: None,
                    inputs: Vec::new(),
                    timeout_ms: Some(30_000),
                },
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();
        assert_eq!(create.exit_code, Some(0), "{}", create.stderr);
        assert!(create.error_code.is_none(), "{:?}", create.error);
        assert!(workspace.path().join(path).is_file());

        let validate = engine
            .execute(
                &workspace_context(workspace.path()),
                &OfficeExecutionRequest {
                    document_kind,
                    operation: OfficeOperation::Validate,
                    document_path: Some(path.to_string()),
                    parameters: OfficeOperationParameters::Validate,
                    output_path: None,
                    destination_path: None,
                    inputs: Vec::new(),
                    timeout_ms: Some(30_000),
                },
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();
        assert_eq!(validate.exit_code, Some(0), "{}", validate.stderr);
        assert!(validate.error_code.is_none(), "{:?}", validate.error);
        assert!(validate.stdout.contains("\"count\": 0"));
    }
}

#[test]
#[ignore = "requires managed OfficeCLI, Office renderer, and core-server component paths"]
fn real_officecli_renders_through_the_application_managed_browser() {
    let officecli = std::env::var_os("MYCOPILOT_OFFICECLI_PATH")
        .expect("MYCOPILOT_OFFICECLI_PATH is required for the render smoke test");
    let renderer = std::env::var_os("MYCOPILOT_OFFICE_RENDERER_DIR")
        .expect("MYCOPILOT_OFFICE_RENDERER_DIR is required for the render smoke test");
    let browser_proxy = std::env::var_os("MYCOPILOT_CORE_SERVER_PATH")
        .expect("MYCOPILOT_CORE_SERVER_PATH is required for the render smoke test");
    let workspace = tempfile::tempdir().unwrap();
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(officecli)
            .with_configured_render_runtime_dir(renderer)
            .with_browser_proxy_executable(browser_proxy)
            .with_workspace_root(workspace.path()),
    )
    .unwrap();

    let create = engine
        .execute(
            &workspace_context(workspace.path()),
            &OfficeExecutionRequest {
                document_kind: OfficeDocumentKind::Document,
                operation: OfficeOperation::Create,
                document_path: Some("managed-render.docx".to_string()),
                parameters: OfficeOperationParameters::Create {
                    locale: None,
                    minimal: false,
                    overwrite: false,
                },
                output_path: None,
                destination_path: None,
                inputs: Vec::new(),
                timeout_ms: Some(30_000),
            },
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(create.error_code.is_none(), "{:?}", create.error);

    let render = engine
        .execute(
            &workspace_context(workspace.path()),
            &OfficeExecutionRequest {
                document_kind: OfficeDocumentKind::Document,
                operation: OfficeOperation::View,
                document_path: Some("managed-render.docx".to_string()),
                parameters: OfficeOperationParameters::View {
                    mode: OfficeViewMode::Screenshot,
                    start: None,
                    end: None,
                    max_lines: None,
                    issue_type: None,
                    limit: None,
                    columns: Vec::new(),
                    pages: vec![OfficePageRange {
                        start: 1,
                        end: Some(1),
                    }],
                    range: None,
                    viewport: None,
                    grid: Some(OfficeGridLayout::Auto),
                    render_mode: Some(OfficeViewRenderMode::Html),
                    page_count: false,
                },
                output_path: Some("managed-render.png".to_string()),
                destination_path: None,
                inputs: Vec::new(),
                timeout_ms: Some(45_000),
            },
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(render.error_code.is_none(), "{:?}", render.error);
    assert!(!render.timed_out);
    assert!(workspace
        .path()
        .join("managed-render.png")
        .metadata()
        .is_ok_and(|metadata| metadata.len() > 1_024));
    assert_eq!(render.outputs.len(), 1);
    let published = &render.outputs[0];
    assert_eq!(
        published.source,
        AgentFileInputRef::Workspace {
            path: "managed-render.png".to_string(),
        }
    );
    assert_eq!(published.mime_type, "image/png");
    assert!(published.readable_by_agent);
    assert_eq!(
        published.page_selection,
        OfficeRenderPageSelection::Explicit { pages: vec![1] }
    );
    let verified = read_verified_agent_file_input(
        Some(workspace.path()),
        AgentPermissions::default(),
        &AgentFileInputExecutionContext::default(),
        &published.source,
        None,
        8 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!(verified.size_bytes, published.size_bytes);
    assert_eq!(verified.sha256, published.sha256);
}

#[test]
#[ignore = "requires managed Office components and MYCOPILOT_NINE_SLIDE_PPTX"]
fn real_nine_slide_presentation_render_verifies_contact_sheet_layout_geometry() {
    let officecli = std::env::var_os("MYCOPILOT_OFFICECLI_PATH")
        .expect("MYCOPILOT_OFFICECLI_PATH is required for the nine-slide render smoke test");
    let renderer = std::env::var_os("MYCOPILOT_OFFICE_RENDERER_DIR")
        .expect("MYCOPILOT_OFFICE_RENDERER_DIR is required for the nine-slide render smoke test");
    let browser_proxy = std::env::var_os("MYCOPILOT_CORE_SERVER_PATH")
        .expect("MYCOPILOT_CORE_SERVER_PATH is required for the nine-slide render smoke test");
    let source = std::env::var_os("MYCOPILOT_NINE_SLIDE_PPTX")
        .expect("MYCOPILOT_NINE_SLIDE_PPTX is required for the nine-slide render smoke test");
    let workspace = tempfile::tempdir().unwrap();
    fs::copy(source, workspace.path().join("nine-slides.pptx")).unwrap();
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(officecli)
            .with_configured_render_runtime_dir(renderer)
            .with_browser_proxy_executable(browser_proxy)
            .with_workspace_root(workspace.path()),
    )
    .unwrap();
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Presentation,
        operation: OfficeOperation::View,
        document_path: Some("nine-slides.pptx".to_string()),
        parameters: OfficeOperationParameters::View {
            mode: OfficeViewMode::Screenshot,
            start: None,
            end: None,
            max_lines: None,
            issue_type: None,
            limit: None,
            columns: Vec::new(),
            pages: Vec::new(),
            range: None,
            viewport: None,
            grid: Some(OfficeGridLayout::Auto),
            render_mode: Some(OfficeViewRenderMode::Auto),
            page_count: false,
        },
        output_path: Some("nine-slides.png".to_string()),
        destination_path: None,
        inputs: Vec::new(),
        timeout_ms: Some(90_000),
    };
    let context = workspace_context(workspace.path());
    let prepared = engine.prepare(&context, &request).unwrap();
    let plan = prepared.resolved_render_plan.as_ref().unwrap();
    assert_eq!(plan.requested_pages, (1..=9).collect::<Vec<_>>());
    assert_eq!(plan.grid, Some(OfficeGridLayout::Columns { columns: 3 }));
    assert_eq!(
        plan.viewport,
        OfficeViewport {
            width: 1600,
            height: 922
        }
    );

    let result = engine
        .execute_prepared(&context, &prepared, AgentCancellationToken::new(), None)
        .unwrap();
    assert!(result.error_code.is_none(), "{:?}", result.error);
    let published = result.outputs.first().expect("published contact sheet");
    assert_eq!((published.width, published.height), (Some(1600), Some(922)));
    let layout = published
        .layout_coverage
        .as_ref()
        .expect("layout geometry receipt");
    assert_eq!(layout.requested_pages, (1..=9).collect::<Vec<_>>());
    assert_eq!(
        layout.evidence,
        crate::office::OfficeRenderLayoutEvidence::TrustedRendererGeometry
    );
    let grid = layout.grid.as_ref().expect("grid geometry");
    assert_eq!((grid.columns, grid.rows), (3, 3));
    assert!(grid.content_width <= grid.viewport_width);
    assert!(grid.content_height <= grid.viewport_height);
}

#[test]
fn presentation_edit_uses_one_stop_on_error_batch_and_publishes_only_the_save_as_copy() {
    let evidence = tempfile::tempdir().unwrap();
    let args_path = evidence.path().join("batch-args.txt");
    let plan_path = evidence.path().join("batch-plan.json");
    let private_batch_path = evidence.path().join("private-batch-path.txt");
    let script = format!(
        r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "batch" ]; then
  printf '' >> "$2" || exit 73
  printf '%s\n' "$@" > "{}"
  /bin/cp "$4" "{}"
  printf '%s/%s\n' "$PWD" "$4" > "{}"
  if ! printf 'provider-mutated-staging' >> "$2"; then exit 73; fi
  printf '{{"success":true}}\n'
  exit 0
fi
if [ "$1" = "validate" ]; then printf '{{"success":true}}\n'; exit 0; fi
exit 64
"#,
        args_path.display(),
        plan_path.display(),
        private_batch_path.display()
    );
    let fixture = Fixture::new(&script);
    let request = frozen_presentation_edit_request(
        &fixture,
        vec![
            presentation_text_set("/slide[1]/shape[@id=41]", "updated"),
            OfficeOperationParameters::Set {
                target: "/slide[1]/shape[@id=44]".to_string(),
                properties: BTreeMap::new(),
                replacement: Some(OfficeTextReplacement {
                    find: "old title".to_string(),
                    replace: "new title".to_string(),
                }),
                force: false,
            },
            OfficeOperationParameters::Swap {
                first_target: "/slide[1]/shape[@id=42]".to_string(),
                second_target: "/slide[1]/shape[@id=43]".to_string(),
            },
            OfficeOperationParameters::Move {
                target: "/slide[1]/shape[@id=45]".to_string(),
                new_parent: None,
                position: Some(OfficeElementPosition::Index { index: 0 }),
                properties: BTreeMap::new(),
            },
        ],
    );
    let source = fixture.workspace.path().join("source.pptx");
    let destination = fixture.workspace.path().join("edited.pptx");
    let source_before = fs::read(&source).unwrap();

    let result = fixture
        .engine
        .execute_presentation_edit(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_eq!(fs::read(&source).unwrap(), source_before);
    let destination_bytes = fs::read(&destination).unwrap();
    assert!(destination_bytes.starts_with(&source_before));
    assert!(destination_bytes.ends_with(b"provider-mutated-staging"));
    let args = fs::read_to_string(args_path).unwrap();
    let args = args.lines().collect::<Vec<_>>();
    assert_eq!(args.first().copied(), Some("batch"));
    assert_eq!(args.get(2).copied(), Some("--input"));
    assert_eq!(args.get(4).copied(), Some("--stop-on-error"));
    assert_eq!(args.get(5).copied(), Some("--json"));
    let plan: serde_json::Value = serde_json::from_slice(&fs::read(plan_path).unwrap()).unwrap();
    let operations = plan.as_array().expect("one atomic OfficeCLI batch array");
    assert_eq!(operations.len(), 4);
    assert_eq!(operations[0]["command"], "set");
    assert_eq!(operations[0]["path"], "/slide[1]/shape[@id=41]");
    assert_eq!(operations[1]["command"], "set");
    assert_eq!(operations[1]["path"], "/slide[1]/shape[@id=44]");
    assert_eq!(operations[1]["props"]["find"], "old title");
    assert_eq!(operations[1]["props"]["replace"], "new title");
    assert_eq!(operations[2]["command"], "swap");
    assert_eq!(operations[2]["path"], "/slide[1]/shape[@id=42]");
    assert_eq!(operations[2]["path2"], "/slide[1]/shape[@id=43]");
    assert_eq!(operations[3]["command"], "move");
    assert_eq!(operations[3]["path"], "/slide[1]/shape[@id=45]");
    assert_eq!(operations[3]["index"], serde_json::json!(0));
    assert!(operations[3]["index"].is_number());
    assert!(operations.iter().all(|operation| {
        operation.get("raw").is_none()
            && operation.get("providerOptions").is_none()
            && operation.get("continueOnError").is_none()
    }));
    let private_batch_path = PathBuf::from(fs::read_to_string(private_batch_path).unwrap().trim());
    assert!(
        !private_batch_path.exists(),
        "the Host-private batch file must be removed after publication"
    );
    assert!(
        !private_batch_path.parent().unwrap().exists(),
        "the entire Host-private edit transaction directory must be removed"
    );
}

#[test]
fn presentation_edit_provider_failure_rolls_back_partial_staging_mutation() {
    let script = r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "batch" ]; then
  printf 'partial-provider-write' >> "$2"
  printf 'failed in %s while editing %s\n' "$PWD" "$2" >&2
  exit 9
fi
exit 64
"#;
    let fixture = Fixture::new(script);
    let request = frozen_presentation_edit_request(
        &fixture,
        vec![presentation_text_set(
            "/slide[1]/shape[@id=404]",
            "stale anchor",
        )],
    );
    let source = fixture.workspace.path().join("source.pptx");
    let destination = fixture.workspace.path().join("edited.pptx");
    let source_before = fs::read(&source).unwrap();

    let result = fixture
        .engine
        .execute_presentation_edit(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert!(result.error_code.is_some());
    assert_eq!(fs::read(&source).unwrap(), source_before);
    assert!(!destination.exists());
    assert!(!result
        .stderr
        .contains(fixture.workspace.path().to_string_lossy().as_ref()));
    assert!(result.stderr.contains("<presentation-edit-private>"));
}

#[test]
fn presentation_edit_strict_validation_failure_never_publishes() {
    let script = r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "batch" ]; then printf '{"success":true}\n'; exit 0; fi
if [ "$1" = "validate" ]; then printf 'schema-invalid\n' >&2; exit 7; fi
exit 64
"#;
    let fixture = Fixture::new(script);
    let request = frozen_presentation_edit_request(
        &fixture,
        vec![presentation_text_set("/slide[1]/shape[@id=41]", "updated")],
    );
    let source = fixture.workspace.path().join("source.pptx");
    let destination = fixture.workspace.path().join("edited.pptx");
    let source_before = fs::read(&source).unwrap();

    let result = fixture
        .engine
        .execute_presentation_edit(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert!(result.error_code.is_some());
    assert!(
        result.stderr.contains("schema-invalid"),
        "unexpected validation stderr: {:?}; error: {:?}",
        result.stderr,
        result.error
    );
    assert_eq!(fs::read(&source).unwrap(), source_before);
    assert!(!destination.exists());
}

#[test]
fn presentation_edit_revalidates_frozen_source_before_starting_the_provider() {
    let evidence = tempfile::tempdir().unwrap();
    let invoked = evidence.path().join("batch-invoked");
    let script = format!(
        r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "batch" ]; then : > "{}"; exit 0; fi
if [ "$1" = "validate" ]; then exit 0; fi
exit 64
"#,
        invoked.display()
    );
    let fixture = Fixture::new(&script);
    let request = frozen_presentation_edit_request(
        &fixture,
        vec![presentation_text_set("/slide[1]/shape[@id=41]", "updated")],
    );
    let source = fixture.workspace.path().join("source.pptx");
    fs::write(&source, b"changed after approval").unwrap();

    let error = fixture
        .engine
        .execute_presentation_edit(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .expect_err("changed approval-time source identity must fail closed");

    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);
    assert!(!invoked.exists());
    assert_eq!(fs::read(&source).unwrap(), b"changed after approval");
    assert!(!fixture.workspace.path().join("edited.pptx").exists());
}

#[test]
fn presentation_edit_requires_a_missing_distinct_save_as_destination() {
    let evidence = tempfile::tempdir().unwrap();
    let invoked = evidence.path().join("batch-invoked");
    let script = format!(
        r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "batch" ]; then : > "{}"; exit 0; fi
if [ "$1" = "validate" ]; then exit 0; fi
exit 64
"#,
        invoked.display()
    );
    let fixture = Fixture::new(&script);
    let mut request = frozen_presentation_edit_request(
        &fixture,
        vec![presentation_text_set("/slide[1]/shape[@id=41]", "updated")],
    );
    let destination = fixture.workspace.path().join("edited.pptx");
    write_pptx(&destination, 1);

    let error = fixture
        .engine
        .execute_presentation_edit(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .expect_err("pre-existing save-as destination must fail closed");
    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);
    assert!(!invoked.exists());

    fs::remove_file(destination).unwrap();
    request.destination_path = request.source_path.clone();
    let error = fixture
        .engine
        .execute_presentation_edit(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .expect_err("in-place editor destination must be rejected");
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
    assert!(!invoked.exists());
}

#[test]
fn presentation_edit_action_cancellation_never_starts_or_publishes_the_batch() {
    let evidence = tempfile::tempdir().unwrap();
    let invoked = evidence.path().join("batch-invoked");
    let script = format!(
        r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "batch" ]; then : > "{}"; printf '{{"success":true}}\n'; exit 0; fi
if [ "$1" = "validate" ]; then printf '{{"success":true}}\n'; exit 0; fi
exit 64
"#,
        invoked.display()
    );
    let fixture = Fixture::new(&script);
    let request = frozen_presentation_edit_request(
        &fixture,
        vec![presentation_text_set("/slide[1]/shape[@id=41]", "updated")],
    );
    let source = fixture.workspace.path().join("source.pptx");
    let source_before = fs::read(&source).unwrap();
    let action_cancel_flag = Arc::new(AtomicBool::new(true));

    let result = fixture
        .engine
        .execute_presentation_edit(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            Some(action_cancel_flag),
        )
        .unwrap();

    assert!(result.cancelled);
    assert_eq!(result.error_code.as_deref(), Some("office.cancelled"));
    assert!(!invoked.exists());
    assert_eq!(fs::read(source).unwrap(), source_before);
    assert!(!fixture.workspace.path().join("edited.pptx").exists());
}

#[test]
fn presentation_edit_picture_uses_only_a_frozen_private_asset_snapshot() {
    let evidence = tempfile::tempdir().unwrap();
    let plan_path = evidence.path().join("batch-plan.json");
    let script = format!(
        r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "batch" ]; then /bin/cp "$4" "{}"; printf '{{"success":true}}\n'; exit 0; fi
if [ "$1" = "validate" ]; then printf '{{"success":true}}\n'; exit 0; fi
exit 64
"#,
        plan_path.display()
    );
    let fixture = Fixture::new(&script);
    let asset_path = fixture.workspace.path().join("replacement.png");
    write_png(&asset_path, 8, 8);
    let asset_spec = AgentFileInputSpec {
        mount_path: "replacement.png".to_string(),
        source: AgentFileInputRef::Workspace {
            path: "replacement.png".to_string(),
        },
    };
    let asset_bindings = prepare_agent_file_input_bindings(
        Some(fixture.workspace.path()),
        AgentPermissions::default(),
        &AgentFileInputExecutionContext::default(),
        std::slice::from_ref(&asset_spec),
        None,
    )
    .unwrap();
    let mut request = frozen_presentation_edit_request(
        &fixture,
        vec![OfficeOperationParameters::Set {
            target: "/slide[1]/picture[@id=51]".to_string(),
            properties: [(
                "src".to_string(),
                serde_json::json!({
                    "resourcePath": office_agent_input_placeholder("replacement.png")
                }),
            )]
            .into_iter()
            .collect(),
            replacement: None,
            force: false,
        }],
    );
    request.inputs = vec![asset_spec];
    request.input_bindings = asset_bindings;
    let source = fixture.workspace.path().join("source.pptx");
    let source_before = fs::read(&source).unwrap();

    let result = fixture
        .engine
        .execute_presentation_edit(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_eq!(fs::read(source).unwrap(), source_before);
    assert!(fixture.workspace.path().join("edited.pptx").exists());
    let plan: serde_json::Value = serde_json::from_slice(&fs::read(plan_path).unwrap()).unwrap();
    let private_asset = plan[0]["props"]["src"]
        .as_str()
        .expect("OfficeCLI receives a private materialized asset path");
    assert!(!private_asset.contains("__mycopilot_office_input__"));
    assert!(Path::new(private_asset).is_absolute());
    assert!(!private_asset.starts_with(fixture.workspace.path().to_string_lossy().as_ref()));
    assert!(
        !Path::new(private_asset).exists(),
        "the private asset snapshot must be removed after the transaction"
    );
}

#[test]
fn presentation_edit_rejects_an_asset_changed_after_approval_before_provider_start() {
    let evidence = tempfile::tempdir().unwrap();
    let invoked = evidence.path().join("batch-invoked");
    let script = format!(
        r#"if [ "$1" = "--version" ]; then printf 'OfficeCLI 1.2.3\n'; exit 0; fi
if [ "$1" = "batch" ]; then : > "{}"; printf '{{"success":true}}\n'; exit 0; fi
if [ "$1" = "validate" ]; then printf '{{"success":true}}\n'; exit 0; fi
exit 64
"#,
        invoked.display()
    );
    let fixture = Fixture::new(&script);
    let asset_path = fixture.workspace.path().join("replacement.png");
    write_png(&asset_path, 8, 8);
    let asset_spec = AgentFileInputSpec {
        mount_path: "replacement.png".to_string(),
        source: AgentFileInputRef::Workspace {
            path: "replacement.png".to_string(),
        },
    };
    let asset_bindings = prepare_agent_file_input_bindings(
        Some(fixture.workspace.path()),
        AgentPermissions::default(),
        &AgentFileInputExecutionContext::default(),
        std::slice::from_ref(&asset_spec),
        None,
    )
    .unwrap();
    let mut request = frozen_presentation_edit_request(
        &fixture,
        vec![OfficeOperationParameters::Set {
            target: "/slide[1]/picture[@id=51]".to_string(),
            properties: [(
                "src".to_string(),
                serde_json::json!({
                    "resourcePath": office_agent_input_placeholder("replacement.png")
                }),
            )]
            .into_iter()
            .collect(),
            replacement: None,
            force: false,
        }],
    );
    request.inputs = vec![asset_spec];
    request.input_bindings = asset_bindings;
    fs::write(&asset_path, b"changed after approval").unwrap();

    let error = fixture
        .engine
        .execute_presentation_edit(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .expect_err("changed edit assets must fail frozen-identity revalidation");

    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);
    assert!(!invoked.exists());
    assert_eq!(fs::read(asset_path).unwrap(), b"changed after approval");
    assert!(!fixture.workspace.path().join("edited.pptx").exists());
}

#[test]
#[ignore = "requires pinned OfficeCLI, renderer, and core-server component paths"]
fn real_presentation_edit_preserves_structure_validates_and_renders_every_page() {
    let officecli = PathBuf::from(
        std::env::var_os("MYCOPILOT_OFFICECLI_PATH").expect("MYCOPILOT_OFFICECLI_PATH is required"),
    );
    let renderer = PathBuf::from(
        std::env::var_os("MYCOPILOT_OFFICE_RENDERER_DIR")
            .expect("MYCOPILOT_OFFICE_RENDERER_DIR is required"),
    );
    let browser_proxy = PathBuf::from(
        std::env::var_os("MYCOPILOT_CORE_SERVER_PATH")
            .expect("MYCOPILOT_CORE_SERVER_PATH is required"),
    );
    let workspace = tempfile::tempdir().unwrap();
    let initial_image = workspace.path().join("initial.png");
    let replacement_image = workspace.path().join("replacement.png");
    write_png(&initial_image, 24, 24);
    let replacement = image::RgbaImage::from_pixel(24, 24, image::Rgba([200, 40, 80, 255]));
    replacement
        .save_with_format(&replacement_image, image::ImageFormat::Png)
        .unwrap();

    let run_cli = |arguments: &[String]| {
        let private_home = workspace.path().join("cli-home");
        fs::create_dir_all(&private_home).unwrap();
        let output = std::process::Command::new(&officecli)
            .args(arguments)
            .current_dir(workspace.path())
            .env_clear()
            .env("HOME", &private_home)
            .env("TMPDIR", &private_home)
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("OFFICECLI_SKIP_UPDATE", "1")
            .env("OFFICECLI_NO_AUTO_RESIDENT", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "real OfficeCLI setup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    run_cli(&[
        "create".to_string(),
        "source.pptx".to_string(),
        "--json".to_string(),
    ]);
    for _ in 0..3 {
        run_cli(&[
            "add".to_string(),
            "source.pptx".to_string(),
            "/".to_string(),
            "--type".to_string(),
            "slide".to_string(),
            "--json".to_string(),
        ]);
    }
    for (slide, id, text) in [(1, 200_001, "Original"), (3, 200_003, "Untouched")] {
        run_cli(&[
            "add".to_string(),
            "source.pptx".to_string(),
            format!("/slide[{slide}]"),
            "--type".to_string(),
            "shape".to_string(),
            "--prop".to_string(),
            format!("id={id}"),
            "--prop".to_string(),
            format!("text={text}"),
            "--prop".to_string(),
            "x=1cm".to_string(),
            "--prop".to_string(),
            "y=1cm".to_string(),
            "--prop".to_string(),
            "width=5cm".to_string(),
            "--prop".to_string(),
            "height=2cm".to_string(),
            "--json".to_string(),
        ]);
    }
    run_cli(&[
        "add".to_string(),
        "source.pptx".to_string(),
        "/slide[2]".to_string(),
        "--type".to_string(),
        "picture".to_string(),
        "--prop".to_string(),
        "id=200002".to_string(),
        "--prop".to_string(),
        format!("src={}", initial_image.display()),
        "--prop".to_string(),
        "x=1cm".to_string(),
        "--prop".to_string(),
        "y=1cm".to_string(),
        "--prop".to_string(),
        "width=4cm".to_string(),
        "--prop".to_string(),
        "height=4cm".to_string(),
        "--json".to_string(),
    ]);

    fn preserved_presentation_parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        let mut parts = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_string();
            if name.starts_with("ppt/slideMasters/")
                || name.starts_with("ppt/slideLayouts/")
                || name.starts_with("ppt/theme/")
            {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
                parts.insert(name, bytes);
            }
        }
        parts
    }

    let source = workspace.path().join("source.pptx");
    let source_before = fs::read(&source).unwrap();
    let preserved_before = preserved_presentation_parts(&source);
    let source_spec = AgentFileInputSpec {
        mount_path: "source.pptx".to_string(),
        source: AgentFileInputRef::Workspace {
            path: "source.pptx".to_string(),
        },
    };
    let image_spec = AgentFileInputSpec {
        mount_path: "replacement.png".to_string(),
        source: AgentFileInputRef::Workspace {
            path: "replacement.png".to_string(),
        },
    };
    let source_binding = prepare_agent_file_input_bindings(
        Some(workspace.path()),
        AgentPermissions::default(),
        &AgentFileInputExecutionContext::default(),
        std::slice::from_ref(&source_spec),
        None,
    )
    .unwrap()
    .remove(0);
    let image_binding = prepare_agent_file_input_bindings(
        Some(workspace.path()),
        AgentPermissions::default(),
        &AgentFileInputExecutionContext::default(),
        std::slice::from_ref(&image_spec),
        None,
    )
    .unwrap()
    .remove(0);
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(&officecli)
            .with_configured_render_runtime_dir(&renderer)
            .with_browser_proxy_executable(&browser_proxy)
            .with_workspace_root(workspace.path()),
    )
    .unwrap();
    let context = workspace_context(workspace.path());
    let destination_binding = prepare_managed_script_binding(
        &context,
        OfficeDocumentKind::Presentation,
        OfficeManagedScriptPurpose::EditPresentationPlan,
        "__mycopilot/presentation-editor/editor.mjs".to_string(),
        Some("source.pptx".to_string()),
        "edited.pptx",
    )
    .unwrap();
    let edit = OfficePresentationEditRequest {
        source_path: "source.pptx".to_string(),
        source_binding,
        destination_path: "edited.pptx".to_string(),
        destination_binding,
        inputs: vec![image_spec],
        input_bindings: vec![image_binding],
        operations: vec![
            OfficeOperationParameters::Set {
                target: "/slide[1]/shape[@id=200001]".to_string(),
                properties: BTreeMap::new(),
                replacement: Some(OfficeTextReplacement {
                    find: "Original".to_string(),
                    replace: "Updated".to_string(),
                }),
                force: false,
            },
            OfficeOperationParameters::Set {
                target: "/slide[2]/picture[@id=200002]".to_string(),
                properties: [(
                    "src".to_string(),
                    serde_json::json!({
                        "resourcePath": office_agent_input_placeholder("replacement.png")
                    }),
                )]
                .into_iter()
                .collect(),
                replacement: None,
                force: false,
            },
        ],
        timeout_ms: Some(120_000),
    };
    let edit_result = engine
        .execute_presentation_edit(&context, &edit, AgentCancellationToken::new(), None)
        .unwrap();
    assert!(
        edit_result.error_code.is_none(),
        "error={:?}; stdout={:?}; stderr={:?}",
        edit_result.error,
        edit_result.stdout,
        edit_result.stderr
    );
    assert_eq!(fs::read(&source).unwrap(), source_before);
    let edited = workspace.path().join("edited.pptx");
    assert_eq!(preserved_presentation_parts(&edited), preserved_before);

    let query = |contains: &str| OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Presentation,
        operation: OfficeOperation::Query,
        document_path: Some("edited.pptx".to_string()),
        parameters: OfficeOperationParameters::Query {
            selector: "shape".to_string(),
            contains: Some(contains.to_string()),
            compact: false,
            fields: Vec::new(),
        },
        output_path: None,
        destination_path: None,
        inputs: Vec::new(),
        timeout_ms: Some(30_000),
    };
    for text in ["Updated", "Untouched"] {
        let result = engine
            .execute(&context, &query(text), AgentCancellationToken::new(), None)
            .unwrap();
        assert!(result.error_code.is_none(), "{:?}", result.error);
        assert!(result.stdout.contains(text));
    }
    let replacement_bytes = fs::read(replacement_image).unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(&edited).unwrap()).unwrap();
    let mut replacement_is_embedded = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.name().starts_with("ppt/media/") {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
            replacement_is_embedded |= bytes == replacement_bytes;
        }
    }
    assert!(replacement_is_embedded);

    let render = engine
        .execute(
            &context,
            &OfficeExecutionRequest {
                document_kind: OfficeDocumentKind::Presentation,
                operation: OfficeOperation::View,
                document_path: Some("edited.pptx".to_string()),
                parameters: OfficeOperationParameters::View {
                    mode: OfficeViewMode::Screenshot,
                    start: None,
                    end: None,
                    max_lines: None,
                    issue_type: None,
                    limit: None,
                    columns: Vec::new(),
                    pages: Vec::new(),
                    range: None,
                    viewport: None,
                    grid: Some(OfficeGridLayout::Auto),
                    render_mode: Some(OfficeViewRenderMode::Auto),
                    page_count: false,
                },
                output_path: Some("edited-preview.png".to_string()),
                destination_path: None,
                inputs: Vec::new(),
                timeout_ms: Some(90_000),
            },
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(render.error_code.is_none(), "{:?}", render.error);
    assert_eq!(
        render.outputs[0]
            .layout_coverage
            .as_ref()
            .unwrap()
            .requested_pages,
        vec![1, 2, 3]
    );
}
