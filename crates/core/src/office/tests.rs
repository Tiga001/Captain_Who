use super::discovery::discover_with_test_path;
use super::execution::{compile_office_arguments, install_commit_test_hook, CommitTestPhase};
use super::types::office_agent_input_placeholder;
use super::*;
use crate::file_input::{
    prepare_agent_file_input_bindings, read_verified_agent_file_input,
    AgentFileInputExecutionContext,
};
use crate::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentCancellationToken,
    AgentFileInputRef, AgentFileInputSpec, AgentInputAttachmentKind, AgentPermissions,
    AgentReadPermission, AgentWritePermission,
};
use std::collections::BTreeMap;
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
            parameters: default_parameters(operation),
            output_path: None,
            destination_path: None,
            inputs: Vec::new(),
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

fn screenshot_request(fixture: &Fixture, output_path: &str) -> OfficeExecutionRequest {
    let mut request = fixture.request(OfficeOperation::View);
    request.parameters = OfficeOperationParameters::View {
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
    };
    request.output_path = Some(output_path.to_string());
    request
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

fn write_png(path: &Path, width: u32, height: u32) {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([24, 48, 72, 255]));
    image
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

fn copy_render_script(path: &Path) -> String {
    format!(
        "#!/bin/sh\nfor output in \"$@\"; do :; done\nprintf 'render-output:%s\\ncwd:%s\\n' \"$output\" \"$PWD\"\n/bin/cp '{}' \"$output\"\n",
        path.display()
    )
}

fn write_pptx(path: &Path, slide_count: u32) {
    let file = fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", options).unwrap();
    archive.write_all(b"<Types/>").unwrap();
    archive.start_file("ppt/presentation.xml", options).unwrap();
    let slide_ids = (1..=slide_count)
        .map(|slide| format!("<p:sldId id=\"{}\" r:id=\"rId{slide}\"/>", 255 + slide))
        .collect::<String>();
    archive
        .write_all(format!("<p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><p:sldIdLst>{slide_ids}</p:sldIdLst><p:sldSz cx=\"12192000\" cy=\"6858000\"/></p:presentation>").as_bytes())
        .unwrap();
    archive
        .start_file("ppt/_rels/presentation.xml.rels", options)
        .unwrap();
    let relationships = (1..=slide_count)
        .map(|slide| format!("<Relationship Id=\"rId{slide}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{slide}.xml\"/>"))
        .collect::<String>();
    archive.write_all(format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>").as_bytes()).unwrap();
    for slide in 1..=slide_count {
        archive
            .start_file(format!("ppt/slides/slide{slide}.xml"), options)
            .unwrap();
        archive.write_all(b"<p:sld xmlns:p=\"urn:test\"/>").unwrap();
    }
    archive.finish().unwrap();
}

fn frozen_presentation_edit_request(
    fixture: &Fixture,
    operations: Vec<OfficeOperationParameters>,
) -> OfficePresentationEditRequest {
    let source_path = fixture.workspace.path().join("source.pptx");
    write_pptx(&source_path, 2);
    let source_spec = AgentFileInputSpec {
        mount_path: "source.pptx".to_string(),
        source: AgentFileInputRef::Workspace {
            path: "source.pptx".to_string(),
        },
    };
    let bindings = prepare_agent_file_input_bindings(
        Some(fixture.workspace.path()),
        AgentPermissions::default(),
        &AgentFileInputExecutionContext::default(),
        std::slice::from_ref(&source_spec),
        None,
    )
    .unwrap();
    let destination_binding = prepare_managed_script_binding(
        &workspace_context(fixture.workspace.path()),
        OfficeDocumentKind::Presentation,
        OfficeManagedScriptPurpose::EditPresentationPlan,
        "__mycopilot/presentation-editor/editor.mjs".to_string(),
        Some("source.pptx".to_string()),
        "edited.pptx",
    )
    .unwrap();
    OfficePresentationEditRequest {
        source_path: "source.pptx".to_string(),
        source_binding: bindings.into_iter().next().unwrap(),
        destination_path: "edited.pptx".to_string(),
        destination_binding,
        inputs: Vec::new(),
        input_bindings: Vec::new(),
        operations,
        timeout_ms: Some(10_000),
    }
}

fn presentation_text_set(target: &str, text: &str) -> OfficeOperationParameters {
    OfficeOperationParameters::Set {
        target: target.to_string(),
        properties: [("text".to_string(), serde_json::json!(text))]
            .into_iter()
            .collect(),
        replacement: None,
        force: false,
    }
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
    request.parameters = OfficeOperationParameters::View {
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
                end: Some(3),
            }],
            range: None,
            viewport: None,
            grid: Some(OfficeGridLayout::Auto),
            render_mode: None,
            page_count: false,
        },
        output_path: Some("preview.png".to_string()),
        destination_path: None,
        inputs: Vec::new(),
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
                end: Some(3),
            }],
            range: None,
            viewport: None,
            grid: Some(OfficeGridLayout::Auto),
            render_mode: None,
            page_count: false,
        },
        output_path: Some("preview.png".to_string()),
        destination_path: None,
        inputs: Vec::new(),
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
        parameters: OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        },
        output_path: None,
        destination_path: None,
        inputs: Vec::new(),
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
fn execution_rechecks_current_permissions() {
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
        parameters: OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        },
        output_path: None,
        destination_path: None,
        inputs: Vec::new(),
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
}

#[test]
fn system_alias_targets_are_supported_only_with_write_all() {
    let fixture = Fixture::new(basic_script());
    let path = format!("@home/.mycopilot-office-{}.docx", uuid::Uuid::new_v4());
    let request = OfficeExecutionRequest {
        document_kind: OfficeDocumentKind::Document,
        operation: OfficeOperation::Create,
        document_path: Some(path),
        parameters: OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        },
        output_path: None,
        destination_path: None,
        inputs: Vec::new(),
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
