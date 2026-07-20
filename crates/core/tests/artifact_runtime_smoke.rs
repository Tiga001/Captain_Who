use mycopilot_core::artifact_runtime::{
    ArtifactRuntimeDiscoveryOptions, ArtifactRuntimeProvider, ARTIFACT_RUNTIME_PROVIDER_ID,
};
use mycopilot_core::command::{
    run_authorized_command_with_artifact_runtime, CommandAuthorizationSource,
};
use mycopilot_core::{
    AgentApprovalStatus, AgentCancellationToken, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationRequest, AgentCommandArtifactValidationStatus,
    AgentCommandExpectedArtifactOutcomeKind, AgentCommandPermission, AgentCommandRequest,
    AgentCommandRuntimeKind, AgentCommandRuntimePackageRequirement, AgentCommandRuntimeProvider,
    AgentCommandRuntimeRequest, AgentCommandSafetyPolicy, AgentPatchPermission, AgentPermissions,
    AgentReadPermission, AgentWritePermission,
};
use std::fs;

#[test]
#[ignore = "release smoke test; requires MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT"]
fn managed_node_and_python_create_observed_office_artifacts() {
    let component = std::env::var_os("MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT")
        .expect("set MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT to a prepared component");
    let provider = ArtifactRuntimeProvider::discover(
        &ArtifactRuntimeDiscoveryOptions::new().with_configured_component_dir(component),
    )
    .expect("discover prepared Artifact Runtime");
    let workspace = tempfile::tempdir().expect("create smoke workspace");

    fs::write(
        workspace.path().join("build-sheet.mjs"),
        r#"import ExcelJS from 'exceljs'
const workbook = new ExcelJS.Workbook()
const sheet = workbook.addWorksheet('Smoke')
sheet.getCell('A1').value = 'managed-node-ok'
await workbook.xlsx.writeFile(process.argv[2])
console.log('created', process.argv[2])
"#,
    )
    .expect("write Node smoke script");
    fs::write(
        workspace.path().join("build-document.py"),
        r#"import sys
from docx import Document
document = Document()
document.add_paragraph('managed-python-ok')
document.save(sys.argv[1])
print('created', sys.argv[1])
"#,
    )
    .expect("write Python smoke script");
    fs::write(
        workspace.path().join("build-presentation.py"),
        r#"import sys
from pptx import Presentation
presentation = Presentation()
slide = presentation.slides.add_slide(presentation.slide_layouts[0])
slide.shapes.title.text = 'managed-python-presentation-ok'
presentation.save(sys.argv[1])
print('created', sys.argv[1])
"#,
    )
    .expect("write Python presentation smoke script");
    fs::write(
        workspace.path().join("build-then-fail.mjs"),
        r#"import ExcelJS from 'exceljs'
const workbook = new ExcelJS.Workbook()
workbook.addWorksheet('Failure evidence').getCell('A1').value = 'created-before-failure'
await workbook.xlsx.writeFile(process.argv[2])
console.error('intentional failure after publish')
process.exitCode = 7
"#,
    )
    .expect("write failing Node smoke script");

    let node = execute(
        workspace.path(),
        &provider,
        "node build-sheet.mjs smoke.xlsx",
        "smoke.xlsx",
        AgentCommandRuntimeKind::Node,
        "exceljs",
        "4.4.0",
    );
    assert_successful_observed_creation(&node, "smoke.xlsx");

    let python = execute(
        workspace.path(),
        &provider,
        "python build-document.py smoke.docx",
        "smoke.docx",
        AgentCommandRuntimeKind::Python,
        "python-docx",
        "1.2.0",
    );
    assert_successful_observed_creation(&python, "smoke.docx");

    let presentation = execute(
        workspace.path(),
        &provider,
        "python build-presentation.py smoke.pptx",
        "smoke.pptx",
        AgentCommandRuntimeKind::Python,
        "python-pptx",
        "1.0.2",
    );
    assert_successful_observed_creation(&presentation, "smoke.pptx");

    let failed = execute(
        workspace.path(),
        &provider,
        "node build-then-fail.mjs failed-but-created.xlsx",
        "failed-but-created.xlsx",
        AgentCommandRuntimeKind::Node,
        "exceljs",
        "4.4.0",
    );
    assert_eq!(failed.exit_code, Some(7));
    assert!(failed.stderr.contains("intentional failure after publish"));
    assert_observed_creation(&failed, "failed-but-created.xlsx");
}

fn execute(
    workspace: &std::path::Path,
    provider: &ArtifactRuntimeProvider,
    command: &str,
    expected_output: &str,
    kind: AgentCommandRuntimeKind,
    package: &str,
    version: &str,
) -> mycopilot_core::command::AgentCommandExecutionResult {
    let request = AgentCommandRequest {
        id: format!("artifact-smoke-{expected_output}"),
        command: command.to_string(),
        cwd: None,
        timeout_ms: Some(30_000),
        approval_status: AgentApprovalStatus::Approved,
        risk_level: None,
        reason: Some("release smoke test".to_string()),
        observe: Some(AgentCommandArtifactObservationRequest {
            kinds: vec![AgentCommandArtifactObservationKind::Office],
            expected_outputs: vec![expected_output.to_string()],
            additional_roots: Vec::new(),
        }),
        runtime: Some(AgentCommandRuntimeRequest {
            provider: AgentCommandRuntimeProvider::ManagedArtifact,
            kind,
            required_packages: vec![AgentCommandRuntimePackageRequirement {
                name: package.to_string(),
                version: version.to_string(),
            }],
        }),
    };
    run_authorized_command_with_artifact_runtime(
        Some(workspace),
        &request,
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::RequireApproval,
        },
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        None,
        Some(provider),
    )
    .expect("execute managed Artifact Runtime command")
}

fn assert_successful_observed_creation(
    result: &mycopilot_core::command::AgentCommandExecutionResult,
    expected_output: &str,
) {
    assert_eq!(result.exit_code, Some(0), "stderr: {}", result.stderr);
    assert!(!result.timed_out);
    assert!(!result.cancelled);
    assert!(result.error.is_none(), "runtime error: {:?}", result.error);
    let runtime = result.runtime.as_ref().expect("runtime evidence");
    assert_eq!(runtime.provider_id, ARTIFACT_RUNTIME_PROVIDER_ID);
    assert!(runtime.error_code.is_none());
    assert!(runtime.bundle_revision.is_some());
    assert!(runtime.runtime_fingerprint.is_some());

    assert_observed_creation(result, expected_output);
}

fn assert_observed_creation(
    result: &mycopilot_core::command::AgentCommandExecutionResult,
    expected_output: &str,
) {
    let observation = result
        .artifact_observation
        .as_ref()
        .expect("artifact observation");
    let expected = observation
        .expected_outputs
        .iter()
        .find(|output| output.requested_path == expected_output)
        .expect("expected output outcome");
    assert_eq!(
        expected.outcome,
        AgentCommandExpectedArtifactOutcomeKind::Created
    );
    assert_eq!(
        expected
            .metadata
            .as_ref()
            .expect("artifact metadata")
            .validation
            .status,
        AgentCommandArtifactValidationStatus::Valid
    );
}
