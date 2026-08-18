use super::*;

#[test]
fn configured_discovery_and_probe_are_structured() {
    let fixture = Fixture::new(basic_script());
    assert_eq!(fixture.engine.source(), OfficeEngineSource::Configured);
    assert!(fixture.engine.executable_path().is_absolute());
    let status = fixture.engine.status(AgentCancellationToken::new());
    assert_eq!(status.availability, OfficeEngineAvailability::Available);
    assert_eq!(status.version.as_deref(), Some("OfficeCLI 1.2.3"));
    assert_eq!(
        status.engine_revision.as_deref(),
        Some(fixture.engine.engine_revision())
    );
    assert!(fixture
        .engine
        .engine_revision()
        .starts_with("office-engine-sha256-v2:"));
    assert_eq!(status.capabilities.document_kinds.len(), 3);
}

#[test]
fn prepared_execution_freezes_engine_workspace_and_file_revisions() {
    let fixture = Fixture::new(basic_script());
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let prepared = fixture
        .engine
        .prepare(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Validate),
        )
        .unwrap();
    assert_eq!(
        prepared.schema_version,
        OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION
    );
    assert_eq!(prepared.engine_revision, fixture.engine.engine_revision());
    assert!(prepared
        .workspace_revision
        .as_deref()
        .unwrap()
        .starts_with("office-workspace-sha256-v1:"));
    assert!(prepared_path(&prepared, &OfficePathSlot::Document)
        .content_revision
        .as_deref()
        .unwrap()
        .starts_with("office-file-sha256-v1:"));
    let mut serialized = serde_json::to_value(&prepared).unwrap();
    assert_eq!(serialized["resolvedRenderPlan"], serde_json::Value::Null);
    serialized
        .as_object_mut()
        .unwrap()
        .remove("resolvedRenderPlan");
    assert!(serde_json::from_value::<OfficePreparedExecution>(serialized).is_err());
}

#[test]
fn changed_engine_or_document_invalidates_prepared_execution() {
    let fixture = Fixture::new(basic_script());
    let document = fixture.workspace.path().join("sample.docx");
    fs::write(&document, b"first").unwrap();
    let request = fixture.request(OfficeOperation::Validate);
    let prepared = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &request)
        .unwrap();
    fs::write(fixture.engine.executable_path(), "#!/bin/sh\nexit 0\n").unwrap();
    let error = fixture
        .engine
        .execute_prepared(
            &workspace_context(fixture.workspace.path()),
            &prepared,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidConfiguration);

    let fresh = Fixture::new(basic_script());
    let document = fresh.workspace.path().join("sample.docx");
    fs::write(&document, b"first").unwrap();
    let prepared = fresh
        .engine
        .prepare(
            &workspace_context(fresh.workspace.path()),
            &fresh.request(OfficeOperation::Validate),
        )
        .unwrap();
    fs::write(&document, b"second").unwrap();
    let error = fresh
        .engine
        .execute_prepared(
            &workspace_context(fresh.workspace.path()),
            &prepared,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);
}

#[test]
fn typed_request_tampering_cannot_reuse_frozen_provider_argv() {
    let fixture = Fixture::new(basic_script());
    let document = fixture.workspace.path().join("sample.docx");
    write_docx(&document, "original");
    let before = fs::read(&document).unwrap();
    let mut prepared = fixture
        .engine
        .prepare(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Set),
        )
        .unwrap();
    let OfficeOperationParameters::Set { properties, .. } = &mut prepared.request.parameters else {
        panic!("fixture must prepare a typed set request");
    };
    properties.insert("text".to_string(), serde_json::json!("tampered"));

    let error = fixture
        .engine
        .execute_prepared(
            &workspace_context(fixture.workspace.path()),
            &prepared,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);
    assert_eq!(fs::read(document).unwrap(), before);
}

#[test]
fn packaged_component_layout_is_discovered_before_path() {
    let workspace = tempfile::tempdir().unwrap();
    let resources = tempfile::tempdir().unwrap();
    let component = resources.path().join(office_cli_component_relative_path());
    fs::create_dir_all(component.parent().unwrap()).unwrap();
    write_executable(&component, basic_script());
    let options = OfficeCliDiscoveryOptions::new()
        .with_application_resources_dir(resources.path())
        .with_workspace_root(workspace.path())
        .allow_path_fallback(true);
    let engine = discover_with_test_path(&options, Some(OsString::from("/missing"))).unwrap();
    assert_eq!(engine.source(), OfficeEngineSource::PackagedComponent);
    assert_eq!(engine.executable_path(), component.canonicalize().unwrap());
}

#[test]
fn path_fallback_is_explicit_and_skips_relative_entries() {
    let workspace = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    write_executable(&path_dir.path().join("officecli"), basic_script());
    let options = OfficeCliDiscoveryOptions::new()
        .with_workspace_root(workspace.path())
        .allow_path_fallback(true);
    let path = std::env::join_paths([PathBuf::from("."), path_dir.path().to_path_buf()]).unwrap();
    let engine = discover_with_test_path(&options, Some(path)).unwrap();
    assert_eq!(engine.source(), OfficeEngineSource::DevelopmentPath);
}

#[test]
fn invalid_explicit_configuration_fails_closed() {
    let resources = tempfile::tempdir().unwrap();
    let component = resources.path().join(office_cli_component_relative_path());
    fs::create_dir_all(component.parent().unwrap()).unwrap();
    write_executable(&component, basic_script());
    let options = OfficeCliDiscoveryOptions::new()
        .with_configured_executable("relative/officecli")
        .with_application_resources_dir(resources.path());
    let error = OfficeCliEngine::discover(&options).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidConfiguration);
}

#[test]
fn executable_inside_workspace_is_rejected() {
    let workspace = tempfile::tempdir().unwrap();
    let executable = workspace.path().join("officecli");
    write_executable(&executable, basic_script());
    let options = OfficeCliDiscoveryOptions::new()
        .with_configured_executable(executable)
        .with_workspace_root(workspace.path());
    let error = OfficeCliEngine::discover(&options).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidConfiguration);
}

#[test]
fn execute_preserves_exit_stdout_and_stderr() {
    let fixture = Fixture::new(
        "#!/bin/sh\nprintf 'created:%s\\n' \"$2\"\nprintf 'warning:%s\\n' \"$3\" >&2\nexit 7\n",
    );
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let request = fixture.request(OfficeOperation::Validate);
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(result.exit_code, Some(7));
    assert!(result.stdout.starts_with("created:"));
    assert!(result.stdout.ends_with("/sample.docx\n"));
    assert_eq!(result.stderr, "warning:--json\n");
    assert_eq!(result.error_code.as_deref(), Some("office.nonzero_exit"));
}

#[test]
fn argv_is_not_interpreted_by_a_shell() {
    let fixture = Fixture::new("#!/bin/sh\nprintf '%s\\n' \"$3\"\n");
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let marker = fixture.workspace.path().join("should-not-exist");
    let mut request = fixture.request(OfficeOperation::Query);
    request.parameters = OfficeOperationParameters::Query {
        selector: format!("$(touch {})", marker.display()),
        contains: None,
        compact: false,
        fields: Vec::new(),
    };
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.stdout.contains("$(touch"));
    assert!(!marker.exists());
}

#[test]
fn timeout_terminates_officecli() {
    let fixture = Fixture::new("#!/bin/sh\n/bin/sleep 5\n");
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let mut request = fixture.request(OfficeOperation::Validate);
    request.timeout_ms = Some(40);
    let started = Instant::now();
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.timed_out);
    assert_eq!(result.error_code.as_deref(), Some("office.timeout"));
    assert!(started.elapsed().as_secs() < 2);
}

#[test]
fn pre_cancelled_request_never_starts_process() {
    let fixture = Fixture::new("#!/bin/sh\ntouch started\n");
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let cancellation = AgentCancellationToken::new();
    cancellation.cancel();
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Validate),
            cancellation,
            None,
        )
        .unwrap();
    assert!(result.cancelled);
    assert!(!fixture.workspace.path().join("started").exists());
}

#[test]
fn managed_environment_disables_updates_and_resident_mode() {
    let fixture = Fixture::new(
        "#!/bin/sh\nprintf '%s:%s\\n' \"$OFFICECLI_SKIP_UPDATE\" \"$OFFICECLI_NO_AUTO_RESIDENT\"\n",
    );
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Validate),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(result.stdout, "1:1\n");
}

#[test]
fn read_only_operations_run_against_a_private_snapshot() {
    let fixture = Fixture::new("#!/bin/sh\nprintf 'corrupt' > \"$2\"\n");
    let document = fixture.workspace.path().join("sample.docx");
    fs::write(&document, b"original").unwrap();
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Validate),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.error_code.is_none());
    assert_eq!(fs::read(document).unwrap(), b"original");
}

#[test]
fn parallel_short_lived_office_process_groups_do_not_cross_signal() {
    const WORKERS: usize = 6;
    const RUNS_PER_WORKER: usize = 4;

    let fixture = Fixture::new("#!/bin/sh\nexit 0\n");
    fs::write(fixture.workspace.path().join("sample.docx"), b"original").unwrap();
    let engine = Arc::new(fixture.engine.clone());
    let workspace = Arc::new(fixture.workspace.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(WORKERS));
    let handles = (0..WORKERS)
        .map(|_| {
            let engine = engine.clone();
            let workspace = workspace.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..RUNS_PER_WORKER {
                    let request = OfficeExecutionRequest {
                        document_kind: OfficeDocumentKind::Document,
                        operation: OfficeOperation::Validate,
                        document_path: Some("sample.docx".to_string()),
                        parameters: OfficeOperationParameters::Validate,
                        output_path: None,
                        destination_path: None,
                        inputs: Vec::new(),
                        timeout_ms: Some(10_000),
                    };
                    let result = engine
                        .execute(
                            &workspace_context(workspace.as_path()),
                            &request,
                            AgentCancellationToken::new(),
                            None,
                        )
                        .unwrap();
                    assert_eq!(result.exit_code, Some(0), "{:?}", result.error);
                    assert!(result.error_code.is_none(), "{:?}", result.error);
                }
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn document_writes_publish_valid_output_atomically() {
    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "replacement");
    let script = format!(
        "#!/bin/sh\n/bin/cp '{}' \"$2\"\n",
        replacement.path().display()
    );
    let fixture = Fixture::new(&script);
    let document = fixture.workspace.path().join("sample.docx");
    write_docx(&document, "original");
    let mut request = fixture.request(OfficeOperation::Set);
    request.parameters = OfficeOperationParameters::Set {
        target: "/body/p[1]".to_string(),
        properties: [("text".to_string(), serde_json::json!("updated"))]
            .into_iter()
            .collect(),
        replacement: None,
        force: false,
    };
    let before = fs::read(&document).unwrap();
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_ne!(fs::read(&document).unwrap(), before);
    assert_eq!(result.engine_revision, fixture.engine.engine_revision());
    assert!(fs::read_dir(fixture.workspace.path())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".mycopilot-office-")));
}

#[test]
fn failed_or_timed_out_writes_never_change_the_target() {
    let fixture = Fixture::new("#!/bin/sh\nprintf 'corrupt' > \"$2\"\nexit 9\n");
    let document = fixture.workspace.path().join("sample.docx");
    write_docx(&document, "original");
    let original = fs::read(&document).unwrap();
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Set),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(result.exit_code, Some(9));
    assert_eq!(fs::read(&document).unwrap(), original);

    let fixture = Fixture::new("#!/bin/sh\nprintf 'corrupt' > \"$2\"\n/bin/sleep 5\n");
    let document = fixture.workspace.path().join("sample.docx");
    write_docx(&document, "original");
    let original = fs::read(&document).unwrap();
    let mut request = fixture.request(OfficeOperation::Set);
    request.timeout_ms = Some(40);
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.timed_out);
    assert_eq!(fs::read(&document).unwrap(), original);
}

#[test]
fn invalid_output_or_concurrent_change_is_never_overwritten() {
    let fixture = Fixture::new("#!/bin/sh\nprintf 'not-ooxml' > \"$2\"\n");
    let document = fixture.workspace.path().join("sample.docx");
    write_docx(&document, "original");
    let original = fs::read(&document).unwrap();
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Set),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(result.error_code.as_deref(), Some("office.invalid_output"));
    assert_eq!(fs::read(&document).unwrap(), original);

    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "replacement");
    let concurrent = tempfile::NamedTempFile::new().unwrap();
    write_docx(concurrent.path(), "concurrent");
    let fixture = Fixture::new(basic_script());
    let document = fixture.workspace.path().join("sample.docx");
    write_docx(&document, "original");
    let concurrent_bytes = fs::read(concurrent.path()).unwrap();
    let script = format!(
        "#!/bin/sh\n/bin/cp '{}' \"$2\"\n/bin/cp '{}' '{}'\n",
        replacement.path().display(),
        concurrent.path().display(),
        document.display()
    );
    write_executable(fixture.engine.executable_path(), &script);
    // Rediscover after installing the final fake so the engine revision is frozen correctly.
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(fixture.engine.executable_path())
            .with_workspace_root(fixture.workspace.path()),
    )
    .unwrap();
    let result = engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Set),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(
        result.error_code.as_deref(),
        Some("office.precondition_failed")
    );
    assert_eq!(fs::read(document).unwrap(), concurrent_bytes);
}

#[test]
fn create_and_render_publish_only_the_valid_staged_artifact() {
    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "created");
    let fixture = Fixture::new(&format!(
        "#!/bin/sh\n/bin/cp '{}' \"$2\"\n",
        replacement.path().display()
    ));
    let mut request = fixture.request(OfficeOperation::Create);
    request.document_path = Some("created.docx".to_string());
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_eq!(
        fs::read(fixture.workspace.path().join("created.docx")).unwrap(),
        fs::read(replacement.path()).unwrap()
    );

    let rendered_png = tempfile::NamedTempFile::new().unwrap();
    write_png(rendered_png.path(), 640, 360);
    let fixture = Fixture::new(&copy_render_script(rendered_png.path()));
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let mut request = fixture.request(OfficeOperation::View);
    request.parameters = OfficeOperationParameters::View {
        mode: OfficeViewMode::Screenshot,
        start: None,
        end: None,
        max_lines: None,
        issue_type: None,
        limit: None,
        columns: Vec::new(),
        pages: vec![OfficePageRange {
            start: 1,
            end: Some(2),
        }],
        range: None,
        viewport: None,
        grid: None,
        render_mode: None,
        page_count: false,
    };
    request.output_path = Some("preview.png".to_string());
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert!(fixture.workspace.path().join("preview.png").is_file());
    assert!(!result.stdout.contains(".mycopilot-office-"));
    assert!(!result.stdout.contains("mycopilot-office-render-"));
    assert!(result.stdout.contains("render-output:preview.png"));
    assert!(
        result.stdout.contains("cwd:<office-render-snapshot>"),
        "{}",
        result.stdout
    );
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(
        serialized.find("\"outputs\"").unwrap() < serialized.find("\"argv\"").unwrap(),
        "model-actionable outputs must precede provider diagnostics"
    );
    let result = serde_json::to_value(result).unwrap();
    let output = &result["outputs"][0];
    assert_eq!(output["role"], "render");
    assert_eq!(output["kind"], "image");
    assert_eq!(output["mimeType"], "image/png");
    assert_eq!(
        output["source"],
        serde_json::json!({ "type": "workspace", "path": "preview.png" })
    );
    assert_eq!(output["readPath"], "preview.png");
    assert_eq!(output["scope"], "workspace");
    assert_eq!(output["readableByAgent"], true);
    assert_eq!(
        output["sizeBytes"],
        fs::metadata(rendered_png.path()).unwrap().len()
    );
    assert!(output["sha256"]
        .as_str()
        .is_some_and(|digest| digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))));
    assert_eq!(output["width"], 640);
    assert_eq!(output["height"], 360);
    assert_eq!(
        output["pageSelection"],
        serde_json::json!({ "type": "explicit", "pages": [1, 2] })
    );
}

#[test]
fn word_pdf_render_uses_only_the_pinned_runtime_and_returns_authoritative_page_count() {
    let rendered_pdf = tempfile::NamedTempFile::new().unwrap();
    write_pdf(rendered_pdf.path(), 3);
    let evidence = tempfile::tempdir().unwrap();
    let officecli_marker = evidence.path().join("officecli-started");
    let renderer_argv = evidence.path().join("renderer-argv");
    let officecli = format!("#!/bin/sh\n: > '{}'\nexit 97\n", officecli_marker.display());
    let renderer = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'runtime:%s\\n' \"$0\"\nprintf 'runtime-neighbor:%s/../share\\n' \"$(/usr/bin/dirname \"$0\")\" >&2\noutdir=''\nprevious=''\nfor argument in \"$@\"; do\n  if [ \"$previous\" = '--outdir' ]; then outdir=\"$argument\"; fi\n  previous=\"$argument\"\ndone\n[ -n \"$outdir\" ] || exit 41\n/bin/cp '{}' \"$outdir/document.pdf\"\n",
        renderer_argv.display(),
        rendered_pdf.path().display(),
    );
    let fixture = Fixture::new_with_word_pdf_renderer(&officecli, renderer.as_bytes());
    fs::write(
        fixture.workspace.path().join("sample.docx"),
        b"frozen-docx-revision",
    )
    .unwrap();
    let uppercase_extension = word_pdf_request(&fixture, "word-qa.PDF");
    let error = validate_office_request(&uppercase_extension).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
    let mut request = fixture.request(OfficeOperation::View);
    request.parameters = OfficeOperationParameters::View {
        mode: OfficeViewMode::Pdf,
        start: None,
        end: None,
        max_lines: None,
        issue_type: None,
        limit: None,
        columns: Vec::new(),
        pages: Vec::new(),
        range: None,
        viewport: None,
        grid: None,
        render_mode: None,
        page_count: false,
    };
    request.output_path = Some("word-qa.pdf".to_string());

    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert!(
        !officecli_marker.exists(),
        "OfficeCLI exporter must not run"
    );
    assert!(fixture.workspace.path().join("word-qa.pdf").is_file());
    assert_eq!(result.outputs.len(), 1);
    let output = &result.outputs[0];
    assert_eq!(output.kind, OfficePublishedOutputKind::Document);
    assert_eq!(output.mime_type, "application/pdf");
    assert_eq!(output.read_path, "word-qa.pdf");
    assert_eq!(output.page_count, Some(3));
    assert!(output
        .source_sha256
        .as_ref()
        .is_some_and(|value| value.len() == 64));
    assert_eq!(
        output.renderer_revision.as_deref(),
        Some(
            fixture
                .engine
                .word_pdf_render_runtime()
                .unwrap()
                .runtime_revision()
        )
    );
    assert_eq!(output.page_selection, OfficeRenderPageSelection::All);
    let argv = fs::read_to_string(renderer_argv).unwrap();
    assert!(argv.contains("pdf:writer_pdf_Export"));
    assert!(argv.contains("--headless"));
    assert!(argv.contains("-env:UserInstallation=file://"));
    assert!(!result.stdout.contains("mycopilot-word-pdf-render-"));
    let runtime_root = fixture._word_pdf_runtime_dir.path().to_string_lossy();
    assert!(!result.stdout.contains(runtime_root.as_ref()));
    assert!(!result.stderr.contains(runtime_root.as_ref()));
    assert!(result.stdout.contains("<word-pdf-runtime>"));
    assert!(result.stderr.contains("<word-pdf-runtime>"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn real_managed_word_pdf_runtime_completes_the_host_transaction_when_configured() {
    let Some(configured_runtime) = std::env::var_os("MYCOPILOT_WORD_PDF_RENDERER_DIR") else {
        return;
    };
    let configured_runtime = PathBuf::from(configured_runtime);
    let configured_runtime = if configured_runtime.is_absolute() {
        configured_runtime
    } else {
        std::env::current_dir().unwrap().join(configured_runtime)
    };
    let configured_runtime = configured_runtime.canonicalize().unwrap();
    managed_artifact_runtime_paths()
        .expect("real Word PDF acceptance also requires MYCOPILOT_ARTIFACT_RUNTIME_DIR");

    let workspace = tempfile::tempdir().unwrap();
    let engine_directory = tempfile::tempdir().unwrap();
    let officecli = engine_directory.path().join("officecli");
    write_executable(&officecli, basic_script());
    let proxy_directory = tempfile::tempdir().unwrap();
    let proxy = proxy_directory.path().join("core-server");
    write_executable(
        &proxy,
        "#!/bin/sh\nprintf 'mycopilot-office-browser-proxy-v1\\n'\n",
    );
    let render_runtime = tempfile::tempdir().unwrap();
    super::render_runtime::write_test_render_runtime(render_runtime.path());
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(&officecli)
            .with_configured_render_runtime_dir(render_runtime.path())
            .with_configured_word_pdf_render_runtime_dir(&configured_runtime)
            .with_browser_proxy_executable(&proxy)
            .with_workspace_root(workspace.path()),
    )
    .unwrap();

    for page_count in [1_u32, 3, 10, 33] {
        let source_name = format!("managed-host-{page_count}.docx");
        let output_name = format!("managed-host-{page_count}.pdf");
        let source = workspace.path().join(&source_name);
        write_renderable_word_pdf_smoke_docx(&source, page_count);
        let request = OfficeExecutionRequest {
            document_kind: OfficeDocumentKind::Document,
            operation: OfficeOperation::View,
            document_path: Some(source_name),
            parameters: OfficeOperationParameters::View {
                mode: OfficeViewMode::Pdf,
                start: None,
                end: None,
                max_lines: None,
                issue_type: None,
                limit: None,
                columns: Vec::new(),
                pages: Vec::new(),
                range: None,
                viewport: None,
                grid: None,
                render_mode: None,
                page_count: false,
            },
            output_path: Some(output_name.clone()),
            destination_path: None,
            inputs: Vec::new(),
            timeout_ms: Some(120_000),
        };
        let result = engine
            .execute(
                &workspace_context(workspace.path()),
                &request,
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();

        assert!(result.error_code.is_none(), "{:?}", result.error);
        assert_eq!(result.outputs.len(), 1);
        assert_eq!(result.outputs[0].read_path, output_name);
        assert_eq!(result.outputs[0].page_count, Some(page_count));
        assert!(result.outputs[0].source_sha256.is_some());
        assert_eq!(
            result.outputs[0].renderer_revision.as_deref(),
            Some(engine.word_pdf_render_runtime().unwrap().runtime_revision())
        );
        let published = workspace.path().join(&result.outputs[0].read_path);
        let pdf = lopdf::Document::load(&published).unwrap();
        assert_eq!(pdf.get_pages().len(), page_count as usize);
        let text = pdf
            .extract_text(&[1, page_count])
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Managed Host transaction page 1"), "{text}");
        assert!(
            text.contains(&format!("Managed Host transaction page {page_count}")),
            "{text}"
        );
        assert!(text.contains(&format!("Page 1 / {page_count}")), "{text}");
        assert!(
            text.contains(&format!("Page {page_count} / {page_count}")),
            "{text}"
        );
        if page_count == 33 {
            assert_managed_pdf_skill_page_coverage(&published, page_count);
        }
    }
    let complex_source = workspace.path().join("managed-host-complex.docx");
    write_complex_word_pdf_acceptance_docx(&complex_source);
    {
        let request = OfficeExecutionRequest {
            document_kind: OfficeDocumentKind::Document,
            operation: OfficeOperation::View,
            document_path: Some("managed-host-complex.docx".to_string()),
            parameters: OfficeOperationParameters::View {
                mode: OfficeViewMode::Pdf,
                start: None,
                end: None,
                max_lines: None,
                issue_type: None,
                limit: None,
                columns: Vec::new(),
                pages: Vec::new(),
                range: None,
                viewport: None,
                grid: None,
                render_mode: None,
                page_count: false,
            },
            output_path: Some("managed-host-complex.pdf".to_string()),
            destination_path: None,
            inputs: Vec::new(),
            timeout_ms: Some(120_000),
        };
        let result = engine
            .execute(
                &workspace_context(workspace.path()),
                &request,
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();
        assert!(result.error_code.is_none(), "{:?}", result.error);
        let page_count = result.outputs[0].page_count.unwrap();
        assert!(
            page_count >= 6,
            "cross-page table did not span enough pages"
        );
        let published = workspace.path().join(&result.outputs[0].read_path);
        let pdf = lopdf::Document::load(&published).unwrap();
        let page_text = (1..=page_count)
            .map(|page| {
                pdf.extract_text(&[page])
                    .unwrap()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>();
        assert!(page_text[0].contains("Roman section marker"));
        assert!(
            page_text[0].contains("Page i / "),
            "first section did not render a lower-Roman PAGE field: {}",
            page_text[0],
        );
        let first_table_page = page_text
            .iter()
            .position(|text| text.contains("Cross-page table row 1 with"))
            .unwrap();
        let last_table_page = page_text
            .iter()
            .position(|text| text.contains("Cross-page table row 180 with"))
            .unwrap();
        assert!(
            last_table_page > first_table_page,
            "table must cross a physical page boundary"
        );
        assert!(
            page_text[first_table_page].contains("Page 1 / "),
            "{}",
            page_text[first_table_page]
        );
        assert!(page_text
            .iter()
            .any(|text| text.contains("After landscape marker")));
        assert!(!page_text.last().unwrap().contains("After landscape marker"));

        let mut has_portrait = false;
        let mut has_landscape = false;
        for object_id in pdf.get_pages().values() {
            let mut current = *object_id;
            loop {
                let dictionary = pdf.get_object(current).unwrap().as_dict().unwrap();
                if let Ok(media_box) = dictionary.get(b"MediaBox") {
                    let bounds = media_box.as_array().unwrap();
                    let width = bounds[2].as_float().unwrap() - bounds[0].as_float().unwrap();
                    let height = bounds[3].as_float().unwrap() - bounds[1].as_float().unwrap();
                    has_portrait |= height > width;
                    has_landscape |= width > height;
                    break;
                }
                current = dictionary.get(b"Parent").unwrap().as_reference().unwrap();
            }
        }
        assert!(has_portrait && has_landscape);
    }
    engine
        .word_pdf_render_runtime()
        .unwrap()
        .verify_integrity()
        .unwrap();
    assert!(
        fs::read_dir(workspace.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".mycopilot-office-")),
        "same-directory staging must be removed after atomic publication"
    );
}

#[test]
fn invalid_word_pdf_candidate_is_not_published_and_never_falls_back_to_officecli() {
    let evidence = tempfile::tempdir().unwrap();
    let officecli_marker = evidence.path().join("officecli-started");
    let officecli = format!("#!/bin/sh\n: > '{}'\nexit 97\n", officecli_marker.display());
    let renderer = b"#!/bin/sh\noutdir=''\nprevious=''\nfor argument in \"$@\"; do\n  if [ \"$previous\" = '--outdir' ]; then outdir=\"$argument\"; fi\n  previous=\"$argument\"\ndone\nprintf 'not-a-pdf' > \"$outdir/document.pdf\"\n";
    let fixture = Fixture::new_with_word_pdf_renderer(&officecli, renderer);
    fs::write(fixture.workspace.path().join("sample.docx"), b"docx").unwrap();
    let mut request = fixture.request(OfficeOperation::View);
    request.parameters = OfficeOperationParameters::View {
        mode: OfficeViewMode::Pdf,
        start: None,
        end: None,
        max_lines: None,
        issue_type: None,
        limit: None,
        columns: Vec::new(),
        pages: Vec::new(),
        range: None,
        viewport: None,
        grid: None,
        render_mode: None,
        page_count: false,
    };
    request.output_path = Some("invalid.pdf".to_string());

    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert_eq!(result.error_code.as_deref(), Some("office.invalid_output"));
    assert!(result.outputs.is_empty());
    assert!(!fixture.workspace.path().join("invalid.pdf").exists());
    assert!(
        !officecli_marker.exists(),
        "OfficeCLI exporter must not run"
    );
}

#[test]
fn word_pdf_process_failures_name_the_managed_renderer_not_officecli() {
    let nonzero = Fixture::new_with_word_pdf_renderer(basic_script(), b"#!/bin/sh\nexit 17\n");
    fs::write(nonzero.workspace.path().join("sample.docx"), b"docx").unwrap();
    let result = nonzero
        .engine
        .execute(
            &workspace_context(nonzero.workspace.path()),
            &word_pdf_request(&nonzero, "nonzero.pdf"),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(result.error_code.as_deref(), Some("office.nonzero_exit"));
    assert_eq!(
        result.error.as_deref(),
        Some("Managed Word PDF renderer exited with non-zero status 17.")
    );
    assert!(!result.error.as_deref().unwrap().contains("OfficeCLI"));

    let timed_out =
        Fixture::new_with_word_pdf_renderer(basic_script(), b"#!/bin/sh\n/bin/sleep 5\n");
    fs::write(timed_out.workspace.path().join("sample.docx"), b"docx").unwrap();
    let mut timeout_request = word_pdf_request(&timed_out, "timeout.pdf");
    timeout_request.timeout_ms = Some(40);
    let result = timed_out
        .engine
        .execute(
            &workspace_context(timed_out.workspace.path()),
            &timeout_request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.timed_out);
    assert_eq!(result.error_code.as_deref(), Some("office.timeout"));
    assert_eq!(
        result.error.as_deref(),
        Some("Managed Word PDF renderer execution exceeded its timeout.")
    );

    let cancelled = Fixture::new_with_word_pdf_renderer(basic_script(), b"#!/bin/sh\nexit 23\n");
    fs::write(cancelled.workspace.path().join("sample.docx"), b"docx").unwrap();
    let cancellation = AgentCancellationToken::new();
    cancellation.cancel();
    let result = cancelled
        .engine
        .execute(
            &workspace_context(cancelled.workspace.path()),
            &word_pdf_request(&cancelled, "cancelled.pdf"),
            cancellation,
            None,
        )
        .unwrap();
    assert!(result.cancelled);
    assert_eq!(result.error_code.as_deref(), Some("office.cancelled"));
    assert_eq!(
        result.error.as_deref(),
        Some("Managed Word PDF renderer execution was cancelled before launch.")
    );
}

#[test]
fn word_pdf_render_preserves_concurrent_output_and_honors_precommit_cancellation() {
    let rendered_pdf = tempfile::NamedTempFile::new().unwrap();
    write_pdf(rendered_pdf.path(), 1);

    let renderer_template = |target: Option<&Path>| {
        format!(
            "#!/bin/sh\noutdir=''\nprevious=''\nfor argument in \"$@\"; do\n  if [ \"$previous\" = '--outdir' ]; then outdir=\"$argument\"; fi\n  previous=\"$argument\"\ndone\n{}\n/bin/cp '{}' \"$outdir/document.pdf\"\n",
            target.map_or_else(String::new, |path| format!(
                "printf 'concurrent-output' > '{}'",
                path.display()
            )),
            rendered_pdf.path().display(),
        )
    };

    // A target created after preparation must win; the private candidate is discarded.
    let mut concurrent =
        Fixture::new_with_word_pdf_renderer(basic_script(), renderer_template(None).as_bytes());
    fs::write(concurrent.workspace.path().join("sample.docx"), b"docx").unwrap();
    let target = concurrent.workspace.path().join("concurrent.pdf");
    // Rebuild only the fake runtime so its conversion process simulates an external actor
    // creating the frozen-missing target while conversion is in flight.
    let renderer = renderer_template(Some(&target));
    let word_runtime = tempfile::tempdir().unwrap();
    super::word_pdf_render_runtime::write_test_word_pdf_render_runtime_with_executable(
        word_runtime.path(),
        renderer.as_bytes(),
    );
    let options = OfficeCliDiscoveryOptions::new()
        .with_configured_executable(concurrent.engine.executable_path())
        .with_configured_render_runtime_dir(concurrent._render_runtime_dir.path())
        .with_configured_word_pdf_render_runtime_dir(word_runtime.path())
        .with_browser_proxy_executable(concurrent._proxy_dir.path().join("core-server"))
        .with_workspace_root(concurrent.workspace.path());
    concurrent.engine = OfficeCliEngine::discover(&options).unwrap();
    let result = concurrent
        .engine
        .execute(
            &workspace_context(concurrent.workspace.path()),
            &word_pdf_request(&concurrent, "concurrent.pdf"),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(
        result.error_code.as_deref(),
        Some("office.precondition_failed")
    );
    assert_eq!(fs::read(&target).unwrap(), b"concurrent-output");
    assert!(result.outputs.is_empty());

    // Cancellation at the existing transaction linearization point must not publish.
    let cancelled =
        Fixture::new_with_word_pdf_renderer(basic_script(), renderer_template(None).as_bytes());
    fs::write(cancelled.workspace.path().join("sample.docx"), b"docx").unwrap();
    let context = workspace_context(cancelled.workspace.path());
    let request = word_pdf_request(&cancelled, "cancel-before-commit.pdf");
    let prepared = cancelled.engine.prepare(&context, &request).unwrap();
    let target = PathBuf::from(&prepared_path(&prepared, &OfficePathSlot::Output).normalized_path);
    let cancellation = Arc::new(AtomicBool::new(false));
    install_commit_test_hook(
        target.clone(),
        CommitTestPhase::BeforeCancellationCheck,
        cancellation.clone(),
    );
    let result = cancelled
        .engine
        .execute_prepared(
            &context,
            &prepared,
            AgentCancellationToken::new(),
            Some(cancellation),
        )
        .unwrap();
    assert!(result.cancelled);
    assert_eq!(result.error_code.as_deref(), Some("office.cancelled"));
    assert_eq!(
        result.error.as_deref(),
        Some("Office operation was cancelled after validation and before the atomic commit.")
    );
    assert!(result.outputs.is_empty());
    assert!(!target.exists());
}

#[test]
fn word_pdf_render_rejects_runtime_mutation_before_publication() {
    let rendered_pdf = tempfile::NamedTempFile::new().unwrap();
    write_pdf(rendered_pdf.path(), 1);
    let renderer = format!(
        "#!/bin/sh\noutdir=''\nprevious=''\nfor argument in \"$@\"; do\n  if [ \"$previous\" = '--outdir' ]; then outdir=\"$argument\"; fi\n  previous=\"$argument\"\ndone\n/bin/cp '{}' \"$outdir/document.pdf\"\nprintf '\\n# mutated after conversion\\n' >> \"$0\"\n",
        rendered_pdf.path().display(),
    );
    let fixture = Fixture::new_with_word_pdf_renderer(basic_script(), renderer.as_bytes());
    fs::write(fixture.workspace.path().join("sample.docx"), b"docx").unwrap();

    let error = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &word_pdf_request(&fixture, "runtime-mutated.pdf"),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();

    assert_eq!(error.code(), OfficeEngineErrorCode::RenderBackendInvalid);
    assert!(!fixture
        .workspace
        .path()
        .join("runtime-mutated.pdf")
        .exists());
}

#[test]
fn failed_timed_out_or_cancelled_render_never_reports_a_published_output() {
    let invalid = Fixture::new(
        "#!/bin/sh\nfor output in \"$@\"; do :; done\nprintf 'not-png' > \"$output\"\n",
    );
    fs::write(invalid.workspace.path().join("sample.docx"), b"doc").unwrap();
    let result = invalid
        .engine
        .execute(
            &workspace_context(invalid.workspace.path()),
            &screenshot_request(&invalid, "invalid.png"),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(result.error_code.as_deref(), Some("office.invalid_output"));
    assert!(serde_json::to_value(&result)
        .unwrap()
        .get("outputs")
        .is_none());
    assert!(!invalid.workspace.path().join("invalid.png").exists());

    let truncated = Fixture::new(
        "#!/bin/sh\nfor output in \"$@\"; do :; done\nprintf '\\211PNG\\r\\n\\032\\n\\000\\000\\000\\rIHDR\\000\\000\\002\\200\\000\\000\\001h' > \"$output\"\n",
    );
    fs::write(truncated.workspace.path().join("sample.docx"), b"doc").unwrap();
    let result = truncated
        .engine
        .execute(
            &workspace_context(truncated.workspace.path()),
            &screenshot_request(&truncated, "truncated.png"),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(result.error_code.as_deref(), Some("office.invalid_output"));
    assert!(result.outputs.is_empty());
    assert!(!truncated.workspace.path().join("truncated.png").exists());

    let timed_out = Fixture::new(
        "#!/bin/sh\nfor output in \"$@\"; do :; done\nprintf '\\211PNG\\r\\n\\032\\npartial' > \"$output\"\n/bin/sleep 5\n",
    );
    fs::write(timed_out.workspace.path().join("sample.docx"), b"doc").unwrap();
    let mut request = screenshot_request(&timed_out, "timed-out.png");
    request.timeout_ms = Some(40);
    let result = timed_out
        .engine
        .execute(
            &workspace_context(timed_out.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.timed_out);
    assert_eq!(result.error_code.as_deref(), Some("office.timeout"));
    assert!(serde_json::to_value(&result)
        .unwrap()
        .get("outputs")
        .is_none());
    assert!(!timed_out.workspace.path().join("timed-out.png").exists());

    let cancelled_png = tempfile::NamedTempFile::new().unwrap();
    write_png(cancelled_png.path(), 640, 360);
    let cancelled = Fixture::new(&copy_render_script(cancelled_png.path()));
    fs::write(cancelled.workspace.path().join("sample.docx"), b"doc").unwrap();
    let context = workspace_context(cancelled.workspace.path());
    let prepared = cancelled
        .engine
        .prepare(&context, &screenshot_request(&cancelled, "cancelled.png"))
        .unwrap();
    let target = PathBuf::from(&prepared_path(&prepared, &OfficePathSlot::Output).normalized_path);
    let cancellation = Arc::new(AtomicBool::new(false));
    install_commit_test_hook(
        target.clone(),
        CommitTestPhase::BeforeCancellationCheck,
        cancellation.clone(),
    );
    let result = cancelled
        .engine
        .execute_prepared(
            &context,
            &prepared,
            AgentCancellationToken::new(),
            Some(cancellation),
        )
        .unwrap();
    assert!(result.cancelled);
    assert_eq!(result.error_code.as_deref(), Some("office.cancelled"));
    assert!(serde_json::to_value(&result)
        .unwrap()
        .get("outputs")
        .is_none());
    assert!(!target.exists());
}

#[test]
fn document_spreadsheet_and_presentation_renders_share_the_published_output_contract() {
    for (document_kind, input_name, output_name) in [
        (
            OfficeDocumentKind::Document,
            "source.docx",
            "document-preview.png",
        ),
        (
            OfficeDocumentKind::Spreadsheet,
            "source.xlsx",
            "spreadsheet-preview.png",
        ),
        (
            OfficeDocumentKind::Presentation,
            "source.pptx",
            "presentation-preview.png",
        ),
    ] {
        let rendered_png = tempfile::NamedTempFile::new().unwrap();
        let dimensions = if document_kind == OfficeDocumentKind::Presentation {
            (1600, 900)
        } else {
            (640, 360)
        };
        write_png(rendered_png.path(), dimensions.0, dimensions.1);
        let fixture = Fixture::new(&copy_render_script(rendered_png.path()));
        if document_kind == OfficeDocumentKind::Presentation {
            write_pptx(&fixture.workspace.path().join(input_name), 1);
        } else {
            fs::write(fixture.workspace.path().join(input_name), b"office-input").unwrap();
        }
        let mut request = screenshot_request(&fixture, output_name);
        request.document_kind = document_kind;
        request.document_path = Some(input_name.to_string());

        let result = fixture
            .engine
            .execute(
                &workspace_context(fixture.workspace.path()),
                &request,
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();

        assert!(result.error_code.is_none(), "{:?}", result.error);
        assert_eq!(result.document_kind, document_kind);
        assert_eq!(result.outputs.len(), 1);
        assert_eq!(result.outputs[0].read_path, output_name);
        assert_eq!(result.outputs[0].scope, OfficePathScope::Workspace);
        assert_eq!(
            result.outputs[0].page_selection,
            OfficeRenderPageSelection::All
        );
        assert!(fixture.workspace.path().join(output_name).is_file());
    }
}

#[test]
fn external_render_output_reports_whether_the_current_read_policy_can_reuse_it() {
    let rendered_png = tempfile::NamedTempFile::new().unwrap();
    write_png(rendered_png.path(), 640, 360);
    let fixture = Fixture::new(&copy_render_script(rendered_png.path()));
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let external = tempfile::tempdir().unwrap();
    let target = fs::canonicalize(external.path())
        .unwrap()
        .join("preview.png");
    let mut request = screenshot_request(&fixture, &target.to_string_lossy());
    request.output_path = Some(target.to_string_lossy().into_owned());

    let result = fixture
        .engine
        .execute(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::All,
            ),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_eq!(result.outputs.len(), 1);
    let output = &result.outputs[0];
    assert_eq!(output.scope, OfficePathScope::External);
    assert!(!output.readable_by_agent);
    assert_eq!(output.read_path, target.to_string_lossy());
    assert_eq!(
        output.source,
        AgentFileInputRef::External {
            path: target.to_string_lossy().into_owned(),
        }
    );
}

#[test]
fn unavailable_engine_is_a_stable_null_object() {
    let options = OfficeCliDiscoveryOptions::new();
    let engine = resolve_office_engine(&options);
    let status = engine.status(AgentCancellationToken::new());
    assert_eq!(status.availability, OfficeEngineAvailability::Unavailable);
    let error = engine
        .prepare(
            &workspace_context(tempfile::tempdir().unwrap().path()),
            &OfficeExecutionRequest {
                document_kind: OfficeDocumentKind::Document,
                operation: OfficeOperation::Help,
                document_path: None,
                parameters: OfficeOperationParameters::Help {
                    verb: None,
                    element: None,
                },
                output_path: None,
                destination_path: None,
                inputs: Vec::new(),
                timeout_ms: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::Unavailable);
}

#[test]
fn unsupported_commands_fail_closed() {
    for command in ["install", "config", "watch", "raw-set", "batch", "mcp"] {
        let error = OfficeOperation::parse_supported(command).unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::UnsafeOperation);
    }
}

#[test]
fn typed_operations_compile_to_deterministic_host_owned_argv() {
    let fixture = Fixture::new(basic_script());
    let expected = [
        (OfficeOperation::Help, vec!["docx", "--json"]),
        (OfficeOperation::Create, vec!["--json"]),
        (OfficeOperation::View, vec!["text", "--json"]),
        (OfficeOperation::Get, vec!["--json"]),
        (OfficeOperation::Query, vec!["*", "--json"]),
        (OfficeOperation::Validate, vec!["--json"]),
        (
            OfficeOperation::Set,
            vec!["/body", "--prop", "text=updated", "--json"],
        ),
        (
            OfficeOperation::Add,
            vec!["/body", "--type", "paragraph", "--json"],
        ),
        (OfficeOperation::Remove, vec!["/body/p[1]", "--json"]),
        (
            OfficeOperation::Move,
            vec!["/body/p[1]", "--index", "0", "--json"],
        ),
        (
            OfficeOperation::Swap,
            vec!["/body/p[1]", "/body/p[2]", "--json"],
        ),
    ];

    for (operation, expected) in expected {
        let request = fixture.request(operation);
        assert_eq!(
            compile_office_arguments(&request).unwrap(),
            expected.into_iter().map(str::to_string).collect::<Vec<_>>(),
            "{}",
            operation.cli_name()
        );
    }

    let mut request = fixture.request(OfficeOperation::Set);
    request.parameters = OfficeOperationParameters::Set {
        target: "/body/p[1]".to_string(),
        properties: [
            ("zeta".to_string(), serde_json::json!(2)),
            ("alpha".to_string(), serde_json::json!(1)),
        ]
        .into_iter()
        .collect(),
        replacement: None,
        force: true,
    };
    assert_eq!(
        compile_office_arguments(&request).unwrap(),
        vec![
            "/body/p[1]",
            "--prop",
            "alpha=1",
            "--prop",
            "zeta=2",
            "--force",
            "--json",
        ]
    );

    let mut contact_sheet = fixture.request(OfficeOperation::View);
    contact_sheet.parameters = OfficeOperationParameters::View {
        mode: OfficeViewMode::Screenshot,
        start: None,
        end: None,
        max_lines: None,
        issue_type: None,
        limit: None,
        columns: Vec::new(),
        pages: vec![OfficePageRange {
            start: 1,
            end: Some(6),
        }],
        range: None,
        viewport: None,
        grid: Some(OfficeGridLayout::Auto),
        render_mode: None,
        page_count: false,
    };
    assert_eq!(
        compile_office_arguments(&contact_sheet).unwrap(),
        vec!["screenshot", "--page", "1-6", "--grid", "auto", "--json",]
    );
}
