use super::discovery::discover_with_test_path;
use super::execution::{compile_office_arguments, install_commit_test_hook, CommitTestPhase};
use super::*;
use crate::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentCancellationToken,
    AgentInputAttachmentKind, AgentPermissions, AgentReadPermission, AgentWritePermission,
};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

struct Fixture {
    workspace: tempfile::TempDir,
    engine_dir: tempfile::TempDir,
    _proxy_dir: tempfile::TempDir,
    _render_runtime_dir: tempfile::TempDir,
    engine: OfficeCliEngine,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let engine_dir = tempfile::tempdir().unwrap();
        let executable = engine_dir.path().join("officecli");
        write_executable(&executable, &fixture_officecli_script(script));
        let proxy_dir = tempfile::tempdir().unwrap();
        let proxy = proxy_dir.path().join("core-server");
        write_executable(
            &proxy,
            "#!/bin/sh\nprintf 'mycopilot-office-browser-proxy-v1\\n'\n",
        );
        let render_runtime_dir = tempfile::tempdir().unwrap();
        super::render_runtime::write_test_render_runtime(render_runtime_dir.path());
        let options = OfficeCliDiscoveryOptions::new()
            .with_configured_executable(&executable)
            .with_configured_render_runtime_dir(render_runtime_dir.path())
            .with_browser_proxy_executable(&proxy)
            .with_workspace_root(workspace.path());
        let engine = OfficeCliEngine::discover(&options).unwrap();
        Self {
            workspace,
            engine_dir,
            _proxy_dir: proxy_dir,
            _render_runtime_dir: render_runtime_dir,
            engine,
        }
    }

    fn request(&self, operation: OfficeOperation) -> OfficeExecutionRequest {
        OfficeExecutionRequest {
            document_kind: OfficeDocumentKind::Document,
            operation,
            document_path: Some("sample.docx".to_string()),
            parameters: OfficeRequestParameters::Typed(default_parameters(operation)),
            output_path: None,
            destination_path: None,
            timeout_ms: Some(10_000),
        }
    }
}

fn fixture_officecli_script(script: &str) -> String {
    format!(
        "#!/bin/sh\nif [ -n \"$MYCOPILOT_OFFICE_BROWSER_FAILURE_MARKER\" ] && [ -n \"$MYCOPILOT_OFFICE_BROWSER_MARKER_NONCE\" ]; then\n  printf '{{\"schemaVersion\":1,\"nonce\":\"%s\",\"status\":\"success\",\"invocationCount\":1,\"maxInvocations\":%s,\"timedOut\":false}}' \"$MYCOPILOT_OFFICE_BROWSER_MARKER_NONCE\" \"$MYCOPILOT_OFFICE_BROWSER_MAX_INVOCATIONS\" > \"$MYCOPILOT_OFFICE_BROWSER_FAILURE_MARKER\"\nfi\n{script}"
    )
}

fn default_parameters(operation: OfficeOperation) -> OfficeOperationParameters {
    match operation {
        OfficeOperation::Help => OfficeOperationParameters::Help {
            verb: None,
            element: None,
        },
        OfficeOperation::Create => OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        },
        OfficeOperation::View => OfficeOperationParameters::View {
            mode: OfficeViewMode::Text,
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
        OfficeOperation::Get => OfficeOperationParameters::Get {
            target: None,
            depth: None,
        },
        OfficeOperation::Query => OfficeOperationParameters::Query {
            selector: "*".to_string(),
            contains: None,
            compact: false,
            fields: Vec::new(),
        },
        OfficeOperation::Validate => OfficeOperationParameters::Validate,
        OfficeOperation::Set => OfficeOperationParameters::Set {
            target: "/body".to_string(),
            properties: [("text".to_string(), serde_json::json!("updated"))]
                .into_iter()
                .collect(),
            replacement: None,
            force: false,
        },
        OfficeOperation::Add => OfficeOperationParameters::Add {
            parent: "/body".to_string(),
            element_type: "paragraph".to_string(),
            copy_from: None,
            position: None,
            properties: std::collections::BTreeMap::new(),
            force: false,
        },
        OfficeOperation::Remove => OfficeOperationParameters::Remove {
            target: "/body/p[1]".to_string(),
            shift: None,
            properties: std::collections::BTreeMap::new(),
        },
        OfficeOperation::Move => OfficeOperationParameters::Move {
            target: "/body/p[1]".to_string(),
            new_parent: None,
            position: Some(OfficeElementPosition::Index { index: 0 }),
            properties: std::collections::BTreeMap::new(),
        },
        OfficeOperation::Swap => OfficeOperationParameters::Swap {
            first_target: "/body/p[1]".to_string(),
            second_target: "/body/p[2]".to_string(),
        },
    }
}

fn workspace_context(path: &Path) -> OfficeExecutionContext {
    let permissions = AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        ..AgentPermissions::default()
    };
    OfficeExecutionContext::new(Some(path.to_path_buf()), permissions, None)
}

fn permission_context(
    workspace: Option<&Path>,
    read: AgentReadPermission,
    write: AgentWritePermission,
) -> OfficeExecutionContext {
    let permissions = AgentPermissions {
        read,
        write,
        ..AgentPermissions::default()
    };
    OfficeExecutionContext::new(workspace.map(Path::to_path_buf), permissions, None)
}

fn prepared_path<'a>(
    prepared: &'a OfficePreparedExecution,
    slot: &OfficePathSlot,
) -> &'a OfficeFrozenPath {
    prepared
        .paths
        .iter()
        .find(|path| &path.slot == slot)
        .expect("prepared Office path slot")
}

fn write_executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).unwrap();
}

fn basic_script() -> &'static str {
    "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'OfficeCLI 1.2.3\\n'; exit 0; fi\nprintf 'stdout:%s:%s\\n' \"$1\" \"$2\"\nprintf 'stderr:%s\\n' \"$3\" >&2\nexit \"${OFFICECLI_TEST_EXIT:-0}\"\n"
}

fn write_docx(path: &Path, text: &str) {
    let file = fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", options).unwrap();
    archive
        .write_all(
            b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>",
        )
        .unwrap();
    archive.start_file("word/document.xml", options).unwrap();
    archive.write_all(text.as_bytes()).unwrap();
    archive.finish().unwrap();
}

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
    serde_json::to_string(&prepared).unwrap();
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
    let OfficeRequestParameters::Typed(OfficeOperationParameters::Set { properties, .. }) =
        &mut prepared.request.parameters
    else {
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
    request.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Query {
        selector: format!("$(touch {})", marker.display()),
        contains: None,
        compact: false,
        fields: Vec::new(),
    });
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
                        parameters: OfficeRequestParameters::Typed(
                            OfficeOperationParameters::Validate,
                        ),
                        output_path: None,
                        destination_path: None,
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
    request.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Set {
        target: "/body/p[1]".to_string(),
        properties: [("text".to_string(), serde_json::json!("updated"))]
            .into_iter()
            .collect(),
        replacement: None,
        force: false,
    });
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

    let fixture = Fixture::new(
        "#!/bin/sh\nfor output in \"$@\"; do :; done\nprintf '\\211PNG\\r\\n\\032\\nbody' > \"$output\"\n",
    );
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let mut request = fixture.request(OfficeOperation::View);
    request.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::View {
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
        grid: None,
        render_mode: None,
        page_count: false,
    });
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
                parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::Help {
                    verb: None,
                    element: None,
                }),
                output_path: None,
                destination_path: None,
                timeout_ms: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::Unavailable);
}

#[test]
fn unsupported_commands_and_legacy_argv_requests_fail_closed() {
    for command in ["install", "config", "watch", "raw-set", "batch", "mcp"] {
        let error = OfficeOperation::parse_supported(command).unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::UnsafeOperation);
    }

    let fixture = Fixture::new(basic_script());
    let mut legacy = fixture.request(OfficeOperation::View);
    legacy.parameters =
        OfficeRequestParameters::Legacy(vec!["text".to_string(), "--browser".to_string()]);
    let error = compile_office_arguments(&legacy).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::UnsupportedOperation);
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
    request.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Set {
        target: "/body/p[1]".to_string(),
        properties: [
            ("zeta".to_string(), serde_json::json!(2)),
            ("alpha".to_string(), serde_json::json!(1)),
        ]
        .into_iter()
        .collect(),
        replacement: None,
        force: true,
    });
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
    contact_sheet.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::View {
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
    });
    assert_eq!(
        compile_office_arguments(&contact_sheet).unwrap(),
        vec!["screenshot", "--page", "1-6", "--grid", "auto", "--json",]
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
    query.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Query {
        selector: "/slide".to_string(),
        contains: None,
        compact: false,
        fields: Vec::new(),
    });
    let error = compile_office_arguments(&query).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
    assert!(error.message().contains("use get"));

    let mut mismatched = fixture.request(OfficeOperation::Validate);
    mismatched.parameters =
        OfficeRequestParameters::Typed(default_parameters(OfficeOperation::Get));
    let error = compile_office_arguments(&mismatched).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    let mut nested_property = fixture.request(OfficeOperation::Set);
    nested_property.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Set {
        target: "/body".to_string(),
        properties: [("text".to_string(), serde_json::json!({"nested": true}))]
            .into_iter()
            .collect(),
        replacement: None,
        force: false,
    });
    let error = compile_office_arguments(&nested_property).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    let mut spreadsheet_view = fixture.request(OfficeOperation::View);
    spreadsheet_view.document_kind = OfficeDocumentKind::Spreadsheet;
    spreadsheet_view.document_path = Some("budget.xlsx".to_string());
    let OfficeRequestParameters::Typed(OfficeOperationParameters::View { mode, grid, .. }) =
        &mut spreadsheet_view.parameters
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
    let OfficeRequestParameters::Typed(OfficeOperationParameters::Query { compact, .. }) =
        &mut spreadsheet_query.parameters
    else {
        panic!("fixture must build a typed query request");
    };
    *compact = true;
    let error = compile_office_arguments(&spreadsheet_query).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    let mut issues = fixture.request(OfficeOperation::View);
    issues.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::View {
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
    });
    let error = compile_office_arguments(&issues).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

    let mut zero_start = fixture.request(OfficeOperation::View);
    let OfficeRequestParameters::Typed(OfficeOperationParameters::View { start, .. }) =
        &mut zero_start.parameters
    else {
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
    request.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Add {
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
    });
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
    request.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Add {
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
    });
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
        request.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Add {
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
        });
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
    background.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Set {
        target: "/body".to_string(),
        properties: [(
            "background".to_string(),
            serde_json::json!({ "resourcePath": "background.png" }),
        )]
        .into_iter()
        .collect(),
        replacement: None,
        force: false,
    });
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
        request.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Set {
            target: "/body".to_string(),
            properties: [(
                property.to_string(),
                serde_json::json!({ "resourcePath": "/etc/passwd" }),
            )]
            .into_iter()
            .collect(),
            replacement: None,
            force: false,
        });
        let error = fixture
            .engine
            .prepare(&workspace_context(fixture.workspace.path()), &request)
            .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    }

    let mut data = fixture.request(OfficeOperation::Add);
    data.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "table".to_string(),
        copy_from: None,
        position: None,
        properties: [("data".to_string(), serde_json::json!("/etc/passwd"))]
            .into_iter()
            .collect(),
        force: false,
    });
    let error = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &data)
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::UnsafeOperation);

    for render in ["auto", "image"] {
        let mut diagram = fixture.request(OfficeOperation::Add);
        diagram.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Add {
            parent: "/body".to_string(),
            element_type: "diagram".to_string(),
            copy_from: None,
            position: None,
            properties: [("render".to_string(), serde_json::json!(render))]
                .into_iter()
                .collect(),
            force: false,
        });
        let error = fixture
            .engine
            .prepare(&workspace_context(fixture.workspace.path()), &diagram)
            .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
    }

    let mut native_diagram = fixture.request(OfficeOperation::Add);
    native_diagram.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::Add {
        parent: "/body".to_string(),
        element_type: "diagram".to_string(),
        copy_from: None,
        position: None,
        properties: std::collections::BTreeMap::new(),
        force: false,
    });
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
        ambiguous_but_non_file.parameters =
            OfficeRequestParameters::Typed(OfficeOperationParameters::Add {
                parent: "/body".to_string(),
                element_type: "shape".to_string(),
                copy_from: None,
                position: None,
                properties: [(name.to_string(), value)].into_iter().collect(),
                force: false,
            });
        fixture
            .engine
            .prepare(
                &workspace_context(fixture.workspace.path()),
                &ambiguous_but_non_file,
            )
            .unwrap();
    }
}

#[test]
fn mutation_destination_is_a_separate_cas_protected_publish_target() {
    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "replacement");
    let fixture = Fixture::new(&format!(
        "#!/bin/sh\n/bin/cp '{}' \"$2\"\n",
        replacement.path().display()
    ));
    let source = fixture.workspace.path().join("sample.docx");
    let destination = fixture.workspace.path().join("copy.docx");
    write_docx(&source, "source");
    let source_before = fs::read(&source).unwrap();
    let mut request = fixture.request(OfficeOperation::Set);
    request.destination_path = Some("copy.docx".to_string());
    let prepared = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &request)
        .unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Document).state,
        OfficeFileState::Present
    );
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Destination).state,
        OfficeFileState::Missing
    );
    let result = fixture
        .engine
        .execute_prepared(
            &workspace_context(fixture.workspace.path()),
            &prepared,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_eq!(fs::read(&source).unwrap(), source_before);
    assert_eq!(
        fs::read(&destination).unwrap(),
        fs::read(replacement.path()).unwrap()
    );

    let second_destination = fixture.workspace.path().join("conflict.docx");
    let mut request = fixture.request(OfficeOperation::Set);
    request.destination_path = Some("conflict.docx".to_string());
    let prepared = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &request)
        .unwrap();
    write_docx(&second_destination, "concurrent-create");
    let concurrent = fs::read(&second_destination).unwrap();
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
    assert_eq!(fs::read(second_destination).unwrap(), concurrent);

    let mut request = fixture.request(OfficeOperation::Set);
    request.destination_path = Some("source-changed.docx".to_string());
    let prepared = fixture
        .engine
        .prepare(&workspace_context(fixture.workspace.path()), &request)
        .unwrap();
    write_docx(&source, "concurrent-source-change");
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
    assert!(!fixture
        .workspace
        .path()
        .join("source-changed.docx")
        .exists());
}

#[test]
fn destination_path_is_rejected_outside_mutation_operations() {
    let fixture = Fixture::new(basic_script());
    write_docx(&fixture.workspace.path().join("sample.docx"), "source");
    for operation in [
        OfficeOperation::Help,
        OfficeOperation::Create,
        OfficeOperation::View,
        OfficeOperation::Get,
        OfficeOperation::Query,
        OfficeOperation::Validate,
    ] {
        let mut request = fixture.request(operation);
        if operation == OfficeOperation::Help {
            request.document_path = None;
        }
        request.destination_path = Some("destination.docx".to_string());
        let error = fixture
            .engine
            .prepare(&workspace_context(fixture.workspace.path()), &request)
            .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
    }
}

#[test]
fn cancellation_is_linearized_before_the_atomic_commit() {
    let replacement = tempfile::NamedTempFile::new().unwrap();
    write_docx(replacement.path(), "replacement");
    let fixture = Fixture::new(&format!(
        "#!/bin/sh\n/bin/cp '{}' \"$2\"\n",
        replacement.path().display()
    ));
    let target = fixture.workspace.path().join("sample.docx");
    write_docx(&target, "original");
    let original = fs::read(&target).unwrap();
    let hook_target = target.canonicalize().unwrap();

    let cancellation = Arc::new(AtomicBool::new(false));
    install_commit_test_hook(
        hook_target.clone(),
        CommitTestPhase::BeforeCancellationCheck,
        cancellation.clone(),
    );
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Set),
            AgentCancellationToken::new(),
            Some(cancellation),
        )
        .unwrap();
    assert!(result.cancelled);
    assert_eq!(result.error_code.as_deref(), Some("office.cancelled"));
    assert_eq!(fs::read(&target).unwrap(), original);

    let cancellation = Arc::new(AtomicBool::new(false));
    install_commit_test_hook(
        hook_target,
        CommitTestPhase::AfterCancellationCheck,
        cancellation.clone(),
    );
    let result = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Set),
            AgentCancellationToken::new(),
            Some(cancellation.clone()),
        )
        .unwrap();
    assert!(cancellation.load(Ordering::SeqCst));
    assert!(!result.cancelled);
    assert!(result.error_code.is_none(), "{:?}", result.error);
    assert_eq!(
        fs::read(target).unwrap(),
        fs::read(replacement.path()).unwrap()
    );
}

#[test]
fn symlinked_inputs_and_component_escapes_are_rejected() {
    let fixture = Fixture::new(basic_script());
    let outside = fixture.engine_dir.path().join("outside.docx");
    fs::write(&outside, b"secret").unwrap();
    symlink(&outside, fixture.workspace.path().join("sample.docx")).unwrap();
    let error = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &fixture.request(OfficeOperation::Validate),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);

    let resources = tempfile::tempdir().unwrap();
    let component = resources.path().join(office_cli_component_relative_path());
    fs::create_dir_all(component.parent().unwrap()).unwrap();
    symlink(fixture.engine.executable_path(), &component).unwrap();
    let options = OfficeCliDiscoveryOptions::new()
        .with_application_resources_dir(resources.path())
        .with_workspace_root(fixture.workspace.path());
    let error = OfficeCliEngine::discover(&options).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidConfiguration);
}

#[test]
fn rendering_requires_a_validated_output_path() {
    let fixture = Fixture::new(basic_script());
    fs::write(fixture.workspace.path().join("sample.docx"), b"doc").unwrap();
    let mut request = fixture.request(OfficeOperation::View);
    request.parameters = OfficeRequestParameters::Typed(OfficeOperationParameters::View {
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
        grid: None,
        render_mode: None,
        page_count: false,
    });
    let error = fixture
        .engine
        .execute(
            &workspace_context(fixture.workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);

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
    assert_eq!(result.argv.last().map(String::as_str), Some("preview.png"));
    assert_eq!(request.access(), OfficeOperationAccess::FileWrite);
}

#[test]
fn missing_managed_browser_fails_before_officecli_starts() {
    let workspace = tempfile::tempdir().unwrap();
    let engine_directory = tempfile::tempdir().unwrap();
    let executable = engine_directory.path().join("officecli");
    let started_marker = workspace.path().join("officecli-started");
    write_executable(
        &executable,
        &format!("#!/bin/sh\ntouch '{}'\n", started_marker.display()),
    );
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(&executable)
            .with_browser_proxy_executable(&executable)
            .with_workspace_root(workspace.path()),
    )
    .unwrap();
    fs::write(workspace.path().join("sample.docx"), b"doc").unwrap();
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::View,
        document_path: Some("sample.docx".to_string()),
        parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::View {
            mode: OfficeViewMode::Screenshot,
            start: None,
            end: None,
            max_lines: None,
            issue_type: None,
            limit: None,
            columns: Vec::new(),
            pages: vec![OfficePageRange {
                start: 1,
                end: Some(3),
            }],
            range: None,
            viewport: None,
            grid: Some(OfficeGridLayout::Auto),
            render_mode: None,
            page_count: false,
        }),
        output_path: Some("preview.png".to_string()),
        destination_path: None,
        timeout_ms: Some(MAX_OFFICE_TIMEOUT_MS),
    };

    let started = Instant::now();
    let error = engine
        .prepare(&workspace_context(workspace.path()), &request)
        .unwrap_err();

    assert_eq!(
        error.code(),
        OfficeEngineErrorCode::RenderBackendUnavailable
    );
    assert_eq!(
        error.code().stable_name(),
        "office.render_backend_unavailable"
    );
    assert!(started.elapsed().as_millis() < 500);
    assert!(!started_marker.exists());
}

#[test]
fn browser_proxy_self_test_rejects_an_unidentified_executable_before_officecli_starts() {
    let workspace = tempfile::tempdir().unwrap();
    let engine_directory = tempfile::tempdir().unwrap();
    let executable = engine_directory.path().join("officecli");
    let started_marker = workspace.path().join("officecli-started");
    write_executable(
        &executable,
        &format!("#!/bin/sh\ntouch '{}'\n", started_marker.display()),
    );
    let proxy_directory = tempfile::tempdir().unwrap();
    let proxy = proxy_directory.path().join("core-server");
    write_executable(&proxy, "#!/bin/sh\nexit 0\n");
    let render_runtime_directory = tempfile::tempdir().unwrap();
    super::render_runtime::write_test_render_runtime(render_runtime_directory.path());
    let engine = OfficeCliEngine::discover(
        &OfficeCliDiscoveryOptions::new()
            .with_configured_executable(&executable)
            .with_configured_render_runtime_dir(render_runtime_directory.path())
            .with_browser_proxy_executable(&proxy)
            .with_workspace_root(workspace.path()),
    )
    .unwrap();
    fs::write(workspace.path().join("sample.docx"), b"doc").unwrap();
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::View,
        document_path: Some("sample.docx".to_string()),
        parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::View {
            mode: OfficeViewMode::Screenshot,
            start: None,
            end: None,
            max_lines: None,
            issue_type: None,
            limit: None,
            columns: Vec::new(),
            pages: vec![OfficePageRange {
                start: 1,
                end: Some(3),
            }],
            range: None,
            viewport: None,
            grid: Some(OfficeGridLayout::Auto),
            render_mode: None,
            page_count: false,
        }),
        output_path: Some("preview.png".to_string()),
        destination_path: None,
        timeout_ms: Some(MAX_OFFICE_TIMEOUT_MS),
    };

    let started = Instant::now();
    let error = engine
        .execute(
            &workspace_context(workspace.path()),
            &request,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();

    assert_eq!(
        error.code(),
        OfficeEngineErrorCode::RenderBackendUnavailable
    );
    assert!(started.elapsed().as_millis() < 2_000);
    assert!(!started_marker.exists());
}

#[test]
fn office_paths_follow_the_read_write_permission_matrix() {
    let fixture = Fixture::new(basic_script());
    let workspace_document = fixture.workspace.path().join("sample.docx");
    write_docx(&workspace_document, "workspace");
    let external = tempfile::tempdir().unwrap();
    let external_root = external.path().canonicalize().unwrap();
    let external_document = external_root.join("external.docx");
    write_docx(&external_document, "external");

    let workspace_read = fixture.request(OfficeOperation::Validate);
    fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::Denied,
            ),
            &workspace_read,
        )
        .expect("write=denied must not remove Office read operations");

    let read = OfficeExecutionRequest {
        document_path: Some(external_document.to_string_lossy().into_owned()),
        ..fixture.request(OfficeOperation::Validate)
    };
    let error = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::WorkspaceOnly,
            ),
            &read,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    let prepared = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::All,
                AgentWritePermission::WorkspaceOnly,
            ),
            &read,
        )
        .unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Document).scope,
        OfficePathScope::External
    );

    let create = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::Create,
        document_path: Some(
            external_root
                .join("created.docx")
                .to_string_lossy()
                .into_owned(),
        ),
        parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        }),
        output_path: None,
        destination_path: None,
        timeout_ms: Some(10_000),
    };
    for write in [
        AgentWritePermission::Denied,
        AgentWritePermission::WorkspaceOnly,
    ] {
        let error = fixture
            .engine
            .prepare(
                &permission_context(
                    Some(fixture.workspace.path()),
                    AgentReadPermission::All,
                    write,
                ),
                &create,
            )
            .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    }
    let prepared = fixture
        .engine
        .prepare(
            &permission_context(
                None,
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::All,
            ),
            &create,
        )
        .unwrap();
    let target = prepared_path(&prepared, &OfficePathSlot::Document);
    assert_eq!(target.scope, OfficePathScope::External);
    assert_eq!(target.purpose, OfficePathPurpose::WriteTarget);
    assert_eq!(
        target.write_disposition,
        Some(OfficeWriteDisposition::CreateNew)
    );
    assert!(target.object_identity.is_none());

    let relative_without_workspace = OfficeExecutionRequest {
        document_path: Some("relative.docx".to_string()),
        ..create.clone()
    };
    let error = fixture
        .engine
        .prepare(
            &permission_context(None, AgentReadPermission::All, AgentWritePermission::All),
            &relative_without_workspace,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
}

#[test]
fn in_place_external_edits_use_write_permission_not_independent_read_permission() {
    let fixture = Fixture::new(basic_script());
    let external = tempfile::tempdir().unwrap();
    let document = external
        .path()
        .canonicalize()
        .unwrap()
        .join("external.docx");
    write_docx(&document, "external");
    let request = OfficeExecutionRequest {
        document_path: Some(document.to_string_lossy().into_owned()),
        ..fixture.request(OfficeOperation::Set)
    };
    let prepared = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::All,
            ),
            &request,
        )
        .unwrap();
    let document = prepared_path(&prepared, &OfficePathSlot::Document);
    assert_eq!(document.purpose, OfficePathPurpose::InPlaceTarget);
    assert_eq!(document.scope, OfficePathScope::External);

    let mut save_as = request;
    save_as.destination_path = Some("copy.docx".to_string());
    let error = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::All,
            ),
            &save_as,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::All,
                AgentWritePermission::WorkspaceOnly,
            ),
            &save_as,
        )
        .unwrap();
}

#[test]
fn registered_attachments_are_read_only_sources() {
    let fixture = Fixture::new(basic_script());
    let library_root = tempfile::tempdir().unwrap();
    let stored = library_root.path().join("stored.docx");
    write_docx(&stored, "attachment");
    let read_path = "@attachments/a1/source.docx".to_string();
    let reference = AgentAttachmentReference {
        id: "a1".to_string(),
        conversation_id: "conversation".to_string(),
        message_id: "message".to_string(),
        project_id: None,
        kind: AgentInputAttachmentKind::File,
        name: "source.docx".to_string(),
        mime_type: Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        ),
        size_bytes: fs::metadata(&stored).unwrap().len(),
        read_path: read_path.clone(),
        storage_rel_path: "stored.docx".to_string(),
        created_at: 1,
    };
    let library = AgentAttachmentLibraryContext {
        root_path: Some(library_root.path().to_string_lossy().into_owned()),
        conversation_id: Some("conversation".to_string()),
        project_id: None,
        conversation_attachments: vec![reference],
        project_attachments: Vec::new(),
    };
    let permissions = AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        ..AgentPermissions::default()
    };
    let context = OfficeExecutionContext::new(
        Some(fixture.workspace.path().to_path_buf()),
        permissions,
        Some(library),
    );
    let request = OfficeExecutionRequest {
        document_path: Some(read_path.clone()),
        ..fixture.request(OfficeOperation::Validate)
    };
    let prepared = fixture.engine.prepare(&context, &request).unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Document).scope,
        OfficePathScope::Attachment
    );

    let mutation = OfficeExecutionRequest {
        document_path: Some(read_path),
        ..fixture.request(OfficeOperation::Set)
    };
    let error = fixture.engine.prepare(&context, &mutation).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
}

#[test]
fn execution_rechecks_current_permissions_and_rejects_schema_v2() {
    let fixture = Fixture::new(basic_script());
    let external = tempfile::tempdir().unwrap();
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::Create,
        document_path: Some(
            external
                .path()
                .canonicalize()
                .unwrap()
                .join("created.docx")
                .to_string_lossy()
                .into_owned(),
        ),
        parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        }),
        output_path: None,
        destination_path: None,
        timeout_ms: Some(10_000),
    };
    let all = permission_context(None, AgentReadPermission::All, AgentWritePermission::All);
    let prepared = fixture.engine.prepare(&all, &request).unwrap();
    let restricted = permission_context(
        Some(fixture.workspace.path()),
        AgentReadPermission::All,
        AgentWritePermission::WorkspaceOnly,
    );
    let error = fixture
        .engine
        .execute_prepared(&restricted, &prepared, AgentCancellationToken::new(), None)
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);

    let mut legacy_json = serde_json::to_value(&prepared).unwrap();
    legacy_json["schemaVersion"] = serde_json::json!(2);
    legacy_json["access"] = serde_json::json!("workspaceWrite");
    legacy_json.as_object_mut().unwrap().remove("paths");
    let legacy: OfficePreparedExecution = serde_json::from_value(legacy_json).unwrap();
    assert_eq!(legacy.access, OfficeOperationAccess::FileWrite);
    let error = fixture
        .engine
        .execute_prepared(&all, &legacy, AgentCancellationToken::new(), None)
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);
}

#[test]
fn system_alias_targets_are_supported_only_with_write_all() {
    let fixture = Fixture::new(basic_script());
    let path = format!("@home/.mycopilot-office-{}.docx", uuid::Uuid::new_v4());
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::Create,
        document_path: Some(path),
        parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        }),
        output_path: None,
        destination_path: None,
        timeout_ms: Some(10_000),
    };
    let error = fixture
        .engine
        .prepare(
            &permission_context(
                Some(fixture.workspace.path()),
                AgentReadPermission::All,
                AgentWritePermission::WorkspaceOnly,
            ),
            &request,
        )
        .unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::WorkspaceViolation);
    let prepared = fixture
        .engine
        .prepare(
            &permission_context(None, AgentReadPermission::All, AgentWritePermission::All),
            &request,
        )
        .unwrap();
    assert_eq!(
        prepared_path(&prepared, &OfficePathSlot::Document).scope,
        OfficePathScope::External
    );
}

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
                    parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::Create {
                        locale: None,
                        minimal: false,
                        overwrite: false,
                    }),
                    output_path: None,
                    destination_path: None,
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
                    parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::Validate),
                    output_path: None,
                    destination_path: None,
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
                parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::Create {
                    locale: None,
                    minimal: false,
                    overwrite: false,
                }),
                output_path: None,
                destination_path: None,
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
                parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::View {
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
                }),
                output_path: Some("managed-render.png".to_string()),
                destination_path: None,
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
}

#[test]
fn schema_v3_argument_snapshots_deserialize_for_retirement_but_never_execute() {
    let fixture = Fixture::new(basic_script());
    let request = fixture.request(OfficeOperation::Validate);
    let mut legacy_json = serde_json::to_value(&request).unwrap();
    legacy_json.as_object_mut().unwrap().remove("parameters");
    legacy_json["arguments"] = serde_json::json!(["--json"]);

    let legacy: OfficeExecutionRequest = serde_json::from_value(legacy_json).unwrap();
    assert!(matches!(
        legacy.parameters,
        OfficeRequestParameters::Legacy(_)
    ));
    let error = compile_office_arguments(&legacy).unwrap_err();
    assert_eq!(error.code(), OfficeEngineErrorCode::UnsupportedOperation);
}
