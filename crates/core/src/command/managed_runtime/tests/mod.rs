use super::*;
use crate::artifact_runtime::{
    ArtifactRuntimeDiscoveryOptions, ArtifactRuntimeKind, ArtifactRuntimeProvider,
    ARTIFACT_RUNTIME_BUNDLE_VERSION, ARTIFACT_RUNTIME_NODE_VERSION, ARTIFACT_RUNTIME_PROVIDER_ID,
    ARTIFACT_RUNTIME_PYTHON_VERSION, ARTIFACT_RUNTIME_RIPGREP_VERSION,
};
use crate::file_input::prepare_agent_file_input_bindings;
use crate::office::{
    OfficeEngineCapabilities, OfficeEngineError, OfficeExecutionRequest, OfficeExecutionResult,
    OfficePreparedExecution,
};
use crate::{
    AgentApprovalStatus, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationRequest, AgentCommandPermission, AgentCommandSafetyPolicy,
    AgentFileInputRef, AgentFileInputSpec, AgentPatchPermission, AgentReadPermission,
    AgentWritePermission,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::atomic::AtomicUsize;
use tempfile::TempDir;

#[test]
fn officecli_schema_diagnostic_keeps_precise_issue_fields() {
    let diagnostic = stable_office_json_diagnostic(
        r#"{"success":false,"issues":[{"type":"schema","description":"Duplicate child element.","path":"/w:tbl/w:tblPr/w:tblLayout[2]","part":"word/document.xml"}]}"#,
        1_024,
    )
    .unwrap();

    assert!(diagnostic.contains("type=schema"));
    assert!(diagnostic.contains("description=Duplicate child element."));
    assert!(diagnostic.contains("path=/w:tbl/w:tblPr/w:tblLayout[2]"));
    assert!(diagnostic.contains("part=word/document.xml"));
}

#[cfg(unix)]
const EDITOR_PLAN_JSON: &str = r#"{"schemaVersion":1,"source":{"type":"input","mountPath":"source.pptx"},"destination":{"type":"output","path":"edited.pptx"},"mode":"saveAs","operations":[{"type":"set","target":"/slide[1]/shape[@id=1]","replacement":{"find":"Old","replace":"New"}}]}"#;

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestComponentReceipt {
    schema_version: u32,
    provider_id: String,
    bundle_version: String,
    build_inputs_revision: String,
    platform: String,
    arch: String,
    runtimes: TestRuntimeSet,
    tools: TestToolSet,
    files: Vec<TestFileReceipt>,
    bundle_revision: String,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestRuntimeSet {
    node: TestRuntimeReceipt,
    python: TestRuntimeReceipt,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestRuntimeReceipt {
    version: String,
    executable: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_home: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bootstrap: Option<String>,
    dependencies: Vec<TestDependencyReceipt>,
    identity_files: Vec<String>,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestDependencyReceipt {
    name: String,
    version: String,
    identity_file: String,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestToolSet {
    pdf_cli: TestPdfCliReceipt,
    ripgrep: TestExecutableToolReceipt,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestPdfCliReceipt {
    version: String,
    path: String,
    identity_files: Vec<String>,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestExecutableToolReceipt {
    version: String,
    executable: String,
    identity_files: Vec<String>,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestFileReceipt {
    path: String,
    size: u64,
    sha256: String,
}

#[cfg(unix)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestReceiptRevisionPayload<'a> {
    schema_version: u32,
    provider_id: &'a str,
    bundle_version: &'a str,
    build_inputs_revision: &'a str,
    platform: &'a str,
    arch: &'a str,
    runtimes: &'a TestRuntimeSet,
    tools: &'a TestToolSet,
    files: &'a [TestFileReceipt],
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum TestOfficeOutcome {
    Success,
    Failure,
    Cancelled,
}

#[cfg(unix)]
struct RecordingPresentationOfficeEngine {
    calls: AtomicUsize,
    outcome: TestOfficeOutcome,
    managed_output_destination: Option<String>,
}

#[cfg(unix)]
impl RecordingPresentationOfficeEngine {
    fn new(outcome: TestOfficeOutcome) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            outcome,
            managed_output_destination: None,
        }
    }

    fn for_managed_output(destination: impl Into<String>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            outcome: TestOfficeOutcome::Success,
            managed_output_destination: Some(destination.into()),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[cfg(unix)]
impl OfficeEngine for RecordingPresentationOfficeEngine {
    fn capabilities(&self) -> OfficeEngineCapabilities {
        OfficeEngineCapabilities::office_cli()
    }

    fn status(&self, _cancellation: AgentCancellationToken) -> crate::office::OfficeEngineStatus {
        panic!("status is not used by the managed-runtime completion hook fixture")
    }

    fn prepare(
        &self,
        _context: &OfficeExecutionContext,
        _request: &OfficeExecutionRequest,
    ) -> Result<OfficePreparedExecution, OfficeEngineError> {
        panic!("prepare is not used by the managed-runtime completion hook fixture")
    }

    fn execute_prepared(
        &self,
        _context: &OfficeExecutionContext,
        _prepared: &OfficePreparedExecution,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        panic!("execute_prepared is not used by the managed-runtime completion hook fixture")
    }

    fn execute_presentation_edit(
        &self,
        context: &OfficeExecutionContext,
        request: &OfficePresentationEditRequest,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficePresentationEditResult, OfficeEngineError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.source_path, "source.pptx");
        assert_eq!(request.source_binding.mount_path, "source.pptx");
        assert_eq!(request.destination_path, "edited.pptx");
        assert_eq!(request.operations.len(), 1);
        match self.outcome {
            TestOfficeOutcome::Success => {
                let destination = context
                    .workspace_root()
                    .expect("editor fixture has a workspace")
                    .join(&request.destination_path);
                fs::write(destination, b"fixture presentation output").unwrap();
                Ok(OfficePresentationEditResult {
                    exit_code: Some(0),
                    stdout: r#"{"success":true,"message":"provider success"}"#.to_string(),
                    stderr: "provider success noise".to_string(),
                    timed_out: false,
                    cancelled: false,
                    duration_ms: 1,
                    error_code: None,
                    error: None,
                })
            }
            TestOfficeOutcome::Failure => Ok(OfficePresentationEditResult {
                exit_code: Some(7),
                stdout: r#"{"success":false,"error":{"code":"invalid_target","error":"Could not find the inspected target.","message":"Re-inspect the deck.","details":"/private/var/folders/secret/presentation-edit-plan.json"}}"#.to_string(),
                stderr:
                    "provider trace /private/var/folders/secret/presentation-edit-plan.json"
                        .to_string(),
                timed_out: false,
                cancelled: false,
                duration_ms: 1,
                error_code: Some("office.fixture_failure".to_string()),
                error: Some("fixture Office failure".to_string()),
            }),
            TestOfficeOutcome::Cancelled => Ok(OfficePresentationEditResult {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                cancelled: true,
                duration_ms: 1,
                error_code: Some("office.cancelled".to_string()),
                error: Some("fixture Office cancellation".to_string()),
            }),
        }
    }

    fn commit_managed_script_output(
        &self,
        context: &OfficeExecutionContext,
        staging: &mut OfficeManagedScriptStaging,
        _managed_python: Option<&ArtifactRuntimeInvocation>,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeManagedScriptOutputResult, OfficeEngineError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let destination = self
            .managed_output_destination
            .as_deref()
            .expect("generic Office fixture declares its publication target");
        let target = context
            .workspace_root()
            .expect("generic Office fixture has a workspace")
            .join(destination);
        fs::rename(staging.candidate_path(), target).unwrap();
        Ok(OfficeManagedScriptOutputResult {
            exit_code: Some(0),
            stdout: r#"{"success":true}"#.to_string(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
            duration_ms: 1,
            error_code: None,
            error: None,
        })
    }
}

#[cfg(unix)]
struct PreparedEditorCompletionFixture {
    _runtime: TempDir,
    workspace: TempDir,
    office: Arc<RecordingPresentationOfficeEngine>,
    plan: CommandSpawnPlan,
    completion_hook: super::super::session::CommandSessionCompletionHook,
}

#[cfg(unix)]
fn test_sha256(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(unix)]
fn test_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

#[cfg(unix)]
fn test_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

#[cfg(unix)]
fn test_dependency(name: &str, version: &str, identity_file: &str) -> TestDependencyReceipt {
    TestDependencyReceipt {
        name: name.to_string(),
        version: version.to_string(),
        identity_file: identity_file.to_string(),
    }
}

#[cfg(unix)]
fn create_test_artifact_runtime() -> (TempDir, Arc<ArtifactRuntimeProvider>) {
    create_test_artifact_runtime_with_python(
        br#"#!/bin/sh
if [ "$1" = "-I" ]; then shift; fi
if [ "$1" = "-B" ]; then shift; fi
if [ "$1" = "-c" ]; then
  shift
  check_program="$1"
  shift
  script="$1"
  case "$check_program" in
*"compile(open(path, 'rb').read(), path, 'exec')"*) ;;
*) printf 'unexpected syntax-check program\n' >&2; exit 2 ;;
  esac
  invalid=0
  while IFS= read -r line || [ -n "$line" ]; do
case "$line" in
  *PYTHON_SYNTAX_ERROR*) invalid=1 ;;
esac
  done < "$script"
  if [ "$invalid" -eq 1 ]; then
printf '  File "%s", line 1\nSyntaxError: fixture\n' "$script" >&2
exit 1
  fi
  exit 0
fi
printf 'builder-ran' > managed-builder-ran.marker
exit 0
"#
        .to_vec(),
    )
}

#[cfg(unix)]
fn create_test_artifact_runtime_with_python(
    python_fixture: Vec<u8>,
) -> (TempDir, Arc<ArtifactRuntimeProvider>) {
    use std::os::unix::fs::PermissionsExt;

    let directory = TempDir::new().unwrap();
    let node_fixture = format!(
        r#"#!/bin/sh
for argument in "$@"; do
  if [ "$argument" = "--check" ]; then
exit 0
  fi
done
if [ -z "$MYCOPILOT_PRESENTATION_EDIT_PLAN" ]; then
  exit 9
fi
printf '%s' '{}' > "$MYCOPILOT_PRESENTATION_EDIT_PLAN"
"#,
        EDITOR_PLAN_JSON
    );
    let mut files = vec![
        ("dependencies/node/bin/node", node_fixture.into_bytes()),
        (
            "dependencies/node/node_modules/docx/package.json",
            br#"{"name":"docx","version":"9.6.1"}"#.to_vec(),
        ),
        (
            "dependencies/node/node_modules/exceljs/package.json",
            br#"{"name":"exceljs","version":"4.4.0"}"#.to_vec(),
        ),
        (
            "dependencies/node/node_modules/pptxgenjs/package.json",
            br#"{"name":"pptxgenjs","version":"4.0.1"}"#.to_vec(),
        ),
        ("dependencies/python/bin/python3", python_fixture),
        ("dependencies/tools/rg", b"ripgrep fixture".to_vec()),
        ("legal/ripgrep/COPYING", b"fixture copyright\n".to_vec()),
        (
            "legal/ripgrep/LICENSE-MIT",
            b"fixture MIT license\n".to_vec(),
        ),
        ("legal/ripgrep/UNLICENSE", b"fixture unlicense\n".to_vec()),
        ("runtime/node-bootstrap.mjs", b"// fixture\n".to_vec()),
        (
            "runtime/pdf-runtime-cli.py",
            b"# managed PDF CLI fixture\n".to_vec(),
        ),
    ];
    for (name, version, path) in [
        (
            "openpyxl",
            "3.1.5",
            "dependencies/python/lib/python3.12/site-packages/openpyxl-3.1.5.dist-info/METADATA",
        ),
        (
            "pdfplumber",
            "0.11.9",
            "dependencies/python/lib/python3.12/site-packages/pdfplumber-0.11.9.dist-info/METADATA",
        ),
        (
            "pypdf",
            "6.15.0",
            "dependencies/python/lib/python3.12/site-packages/pypdf-6.15.0.dist-info/METADATA",
        ),
        (
            "pypdfium2",
            "5.12.1",
            "dependencies/python/lib/python3.12/site-packages/pypdfium2-5.12.1.dist-info/METADATA",
        ),
        (
            "python-docx",
            "1.2.0",
            "dependencies/python/lib/python3.12/site-packages/python_docx-1.2.0.dist-info/METADATA",
        ),
        (
            "python-pptx",
            "1.0.2",
            "dependencies/python/lib/python3.12/site-packages/python_pptx-1.0.2.dist-info/METADATA",
        ),
        (
            "reportlab",
            "4.4.9",
            "dependencies/python/lib/python3.12/site-packages/reportlab-4.4.9.dist-info/METADATA",
        ),
        (
            "XlsxWriter",
            "3.2.9",
            "dependencies/python/lib/python3.12/site-packages/xlsxwriter-3.2.9.dist-info/METADATA",
        ),
    ] {
        files.push((
            path,
            format!("Name: {name}\nVersion: {version}\n").into_bytes(),
        ));
    }
    files.sort_by_key(|(path, _)| *path);
    for (relative, bytes) in &files {
        let path = directory.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        if relative.ends_with("/node")
            || relative.ends_with("/python3")
            || relative.ends_with("/rg")
        {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let file_receipts = files
        .iter()
        .map(|(path, bytes)| TestFileReceipt {
            path: (*path).to_string(),
            size: bytes.len() as u64,
            sha256: test_sha256(bytes),
        })
        .collect::<Vec<_>>();
    let node_dependencies = vec![
        test_dependency(
            "docx",
            "9.6.1",
            "dependencies/node/node_modules/docx/package.json",
        ),
        test_dependency(
            "exceljs",
            "4.4.0",
            "dependencies/node/node_modules/exceljs/package.json",
        ),
        test_dependency(
            "pptxgenjs",
            "4.0.1",
            "dependencies/node/node_modules/pptxgenjs/package.json",
        ),
    ];
    let python_dependencies = vec![
        test_dependency(
            "openpyxl",
            "3.1.5",
            "dependencies/python/lib/python3.12/site-packages/openpyxl-3.1.5.dist-info/METADATA",
        ),
        test_dependency(
            "pdfplumber",
            "0.11.9",
            "dependencies/python/lib/python3.12/site-packages/pdfplumber-0.11.9.dist-info/METADATA",
        ),
        test_dependency(
            "pypdf",
            "6.15.0",
            "dependencies/python/lib/python3.12/site-packages/pypdf-6.15.0.dist-info/METADATA",
        ),
        test_dependency(
            "pypdfium2",
            "5.12.1",
            "dependencies/python/lib/python3.12/site-packages/pypdfium2-5.12.1.dist-info/METADATA",
        ),
        test_dependency(
            "python-docx",
            "1.2.0",
            "dependencies/python/lib/python3.12/site-packages/python_docx-1.2.0.dist-info/METADATA",
        ),
        test_dependency(
            "python-pptx",
            "1.0.2",
            "dependencies/python/lib/python3.12/site-packages/python_pptx-1.0.2.dist-info/METADATA",
        ),
        test_dependency(
            "reportlab",
            "4.4.9",
            "dependencies/python/lib/python3.12/site-packages/reportlab-4.4.9.dist-info/METADATA",
        ),
        test_dependency(
            "xlsxwriter",
            "3.2.9",
            "dependencies/python/lib/python3.12/site-packages/xlsxwriter-3.2.9.dist-info/METADATA",
        ),
    ];
    let mut receipt = TestComponentReceipt {
        schema_version: 3,
        provider_id: ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
        bundle_version: ARTIFACT_RUNTIME_BUNDLE_VERSION.to_string(),
        build_inputs_revision: format!(
            "artifact-runtime-build-inputs-sha256-v1:{}",
            "a".repeat(64)
        ),
        platform: test_platform().to_string(),
        arch: test_arch().to_string(),
        runtimes: TestRuntimeSet {
            node: TestRuntimeReceipt {
                version: ARTIFACT_RUNTIME_NODE_VERSION.to_string(),
                executable: "dependencies/node/bin/node".to_string(),
                package_root: Some("dependencies/node/node_modules".to_string()),
                runtime_home: None,
                bootstrap: Some("runtime/node-bootstrap.mjs".to_string()),
                identity_files: std::iter::once("dependencies/node/bin/node".to_string())
                    .chain(std::iter::once("runtime/node-bootstrap.mjs".to_string()))
                    .chain(
                        node_dependencies
                            .iter()
                            .map(|dependency| dependency.identity_file.clone()),
                    )
                    .collect(),
                dependencies: node_dependencies,
            },
            python: TestRuntimeReceipt {
                version: ARTIFACT_RUNTIME_PYTHON_VERSION.to_string(),
                executable: "dependencies/python/bin/python3".to_string(),
                package_root: None,
                runtime_home: Some("dependencies/python".to_string()),
                bootstrap: None,
                identity_files: std::iter::once("dependencies/python/bin/python3".to_string())
                    .chain(
                        python_dependencies
                            .iter()
                            .map(|dependency| dependency.identity_file.clone()),
                    )
                    .collect(),
                dependencies: python_dependencies,
            },
        },
        tools: TestToolSet {
            pdf_cli: TestPdfCliReceipt {
                version: "1".to_string(),
                path: "runtime/pdf-runtime-cli.py".to_string(),
                identity_files: vec!["runtime/pdf-runtime-cli.py".to_string()],
            },
            ripgrep: TestExecutableToolReceipt {
                version: ARTIFACT_RUNTIME_RIPGREP_VERSION.to_string(),
                executable: "dependencies/tools/rg".to_string(),
                identity_files: vec![
                    "dependencies/tools/rg".to_string(),
                    "legal/ripgrep/COPYING".to_string(),
                    "legal/ripgrep/LICENSE-MIT".to_string(),
                    "legal/ripgrep/UNLICENSE".to_string(),
                ],
            },
        },
        files: file_receipts,
        bundle_revision: String::new(),
    };
    let revision_payload = TestReceiptRevisionPayload {
        schema_version: receipt.schema_version,
        provider_id: &receipt.provider_id,
        bundle_version: &receipt.bundle_version,
        build_inputs_revision: &receipt.build_inputs_revision,
        platform: &receipt.platform,
        arch: &receipt.arch,
        runtimes: &receipt.runtimes,
        tools: &receipt.tools,
        files: &receipt.files,
    };
    receipt.bundle_revision = format!(
        "artifact-runtime-bundle-sha256-v1:{}",
        test_sha256(&serde_json::to_vec(&revision_payload).unwrap())
    );
    fs::write(
        directory.path().join("component-receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    let provider = Arc::new(
        ArtifactRuntimeProvider::discover(
            &ArtifactRuntimeDiscoveryOptions::new().with_configured_component_dir(directory.path()),
        )
        .unwrap(),
    );
    (directory, provider)
}

#[cfg(unix)]
fn office_editor_test_python() -> Option<PathBuf> {
    let mut candidates = std::env::var_os("MYCOPILOT_OFFICE_TEST_PYTHON")
        .map(PathBuf::from)
        .into_iter()
        .collect::<Vec<_>>();
    candidates.extend([
        PathBuf::from("/opt/miniconda3/bin/python3"),
        PathBuf::from("/usr/local/bin/python3"),
        PathBuf::from("/usr/bin/python3"),
    ]);
    candidates.into_iter().find(|candidate| {
        candidate.is_file()
            && std::process::Command::new(candidate)
                .args([
                    "-c",
                    "import docx, openpyxl; assert docx.__version__; assert openpyxl.__version__",
                ])
                .status()
                .is_ok_and(|status| status.success())
    })
}

#[cfg(unix)]
fn real_python_component_fixture(python: &Path) -> Vec<u8> {
    let quoted = python.to_string_lossy().replace('\'', "'\"'\"'");
    format!("#!/bin/sh\nunset PYTHONHOME PYTHONPATH\nexec '{quoted}' \"$@\"\n").into_bytes()
}

#[cfg(unix)]
fn editor_permissions() -> AgentPermissions {
    AgentPermissions {
        read: AgentReadPermission::WorkspaceOnly,
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::RequireApproval,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        patch: AgentPatchPermission::RequireApproval,
    }
}

#[cfg(unix)]
fn prepare_editor_completion_fixture(
    outcome: TestOfficeOutcome,
) -> PreparedEditorCompletionFixture {
    let workspace = TempDir::new().unwrap();
    fs::create_dir_all(workspace.path().join("scripts")).unwrap();
    fs::write(
        workspace.path().join("scripts/editor.mjs"),
        include_str!("../../../skills/bundled/presentations/templates/editor.mjs"),
    )
    .unwrap();
    fs::write(workspace.path().join("source.pptx"), b"fixture source").unwrap();
    let input_context = AgentFileInputExecutionContext::default();
    let permissions = editor_permissions();
    let inputs = prepare_agent_file_input_bindings(
        Some(workspace.path()),
        permissions,
        &input_context,
        &[
            AgentFileInputSpec {
                mount_path: super::super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH.to_string(),
                source: AgentFileInputRef::Workspace {
                    path: "scripts/editor.mjs".to_string(),
                },
            },
            AgentFileInputSpec {
                mount_path: "source.pptx".to_string(),
                source: AgentFileInputRef::Workspace {
                    path: "source.pptx".to_string(),
                },
            },
        ],
        None,
    )
    .unwrap();
    let (runtime, provider) = create_test_artifact_runtime();
    let binding = super::super::prepare_command_runtime_profile(
        &provider,
        AgentCommandRuntimeProfile::Presentations,
        AgentCommandRuntimeKind::Node,
    )
    .unwrap()
    .binding;
    let destination_binding = crate::office::prepare_managed_script_binding(
        &OfficeExecutionContext::new(Some(workspace.path().to_path_buf()), permissions, None),
        crate::office::OfficeDocumentKind::Presentation,
        OfficeManagedScriptPurpose::EditPresentationPlan,
        super::super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH.to_string(),
        Some("source.pptx".to_string()),
        "edited.pptx",
    )
    .unwrap();
    let request = AgentCommandRequest {
        id: "presentation-editor-completion".to_string(),
        command: "node scripts/editor.mjs --source source.pptx --output edited.pptx".to_string(),
        cwd: None,
        timeout_ms: Some(5_000),
        approval_status: AgentApprovalStatus::Approved,
        risk_level: None,
        reason: None,
        observe: Some(AgentCommandArtifactObservationRequest {
            kinds: vec![AgentCommandArtifactObservationKind::Office],
            expected_outputs: vec!["edited.pptx".to_string()],
            additional_roots: Vec::new(),
        }),
        inputs,
        runtime_binding: Some(Box::new(binding)),
        managed_office_script: Some(Box::new(destination_binding)),
    };
    let office = Arc::new(RecordingPresentationOfficeEngine::new(outcome));
    let preparation = prepare_managed_command_session(
        Some(workspace.path()),
        &request,
        permissions,
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        ManagedCommandSessionServices {
            artifact_runtime: Some(provider),
            office_engine: Some(office.clone()),
            file_inputs: Some(&input_context),
            managed_workspace: None,
        },
    )
    .unwrap();
    let ManagedCommandSessionPreparation::Ready {
        plan,
        completion_hook,
    } = preparation
    else {
        panic!("editor fixture unexpectedly failed during preparation")
    };
    PreparedEditorCompletionFixture {
        _runtime: runtime,
        workspace,
        office,
        plan,
        completion_hook,
    }
}

#[cfg(unix)]
fn completion_result(
    plan: &CommandSpawnPlan,
    exit_code: Option<i32>,
) -> AgentCommandExecutionResult {
    AgentCommandExecutionResult {
        outputs: Vec::new(),
        command: plan.command().to_string(),
        cwd: plan.cwd_projection().to_string(),
        exit_code,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: false,
        duration_ms: 1,
        stdout_truncated: false,
        stderr_truncated: false,
        output_capture: ProcessOutputCaptureMetadata::default(),
        stdout_spool: ProcessOutputSpool::default(),
        stderr_spool: ProcessOutputSpool::default(),
        error: None,
        policy_evaluation: None,
        artifact_observation: None,
        input_files: Vec::new(),
        runtime: None,
        managed_outputs: None,
        authoritative_archive_ref: None,
        history_open: None,
    }
}

mod office_transactions;
mod pdf_and_scope;
mod syntax_checks;
