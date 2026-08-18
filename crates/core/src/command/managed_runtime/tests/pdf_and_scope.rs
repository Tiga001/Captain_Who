use super::*;

#[test]
fn managed_pdf_uses_one_host_owned_hard_timeout() {
    let expected = Some(Duration::from_millis(MANAGED_PDF_HARD_TIMEOUT_MS));
    assert_eq!(managed_command_hard_timeout(true, false, None), expected);
    assert_eq!(managed_command_hard_timeout(true, true, Some(1)), expected);
    assert_eq!(managed_command_hard_timeout(false, false, None), None);
    assert_eq!(
        managed_command_hard_timeout(false, false, Some(MAX_TIMEOUT_MS + 1)),
        Some(Duration::from_millis(MAX_TIMEOUT_MS))
    );
    assert_eq!(
        managed_command_hard_timeout(false, true, None),
        Some(Duration::from_millis(PRESENTATION_EDITOR_PLAN_TIMEOUT_MS))
    );
    assert_eq!(
        managed_command_hard_timeout(false, true, Some(250)),
        Some(Duration::from_millis(250))
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "release smoke test; requires MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT"]
fn managed_pdf_shell_runs_receipt_tools_in_one_outer_sandbox() {
    let component = std::env::var_os("MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT")
        .expect("set MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT");
    let provider = ArtifactRuntimeProvider::discover(
        &crate::artifact_runtime::ArtifactRuntimeDiscoveryOptions::new()
            .with_configured_component_dir(component),
    )
    .unwrap();
    let prepared = super::prepare_command_runtime_profile(
        &provider,
        AgentCommandRuntimeProfile::Pdf,
        AgentCommandRuntimeKind::Python,
    )
    .unwrap();
    let fixture = TempDir::new().unwrap();
    let workspace = ManagedCommandWorkspaceLease::open(fixture.path().join("managed-run")).unwrap();
    let external = fixture.path().join("external-secret.txt");
    fs::write(&external, b"secret").unwrap();
    let script = format!(
        r#"python - <<'PY'
import os, socket, subprocess
from pathlib import Path
from reportlab.pdfgen import canvas
from pypdf import PdfReader, PdfWriter
import pdfplumber
import pypdfium2 as pdfium

assert os.environ.get('PATH') == ''
assert os.environ.get('HOME') == '.home'
assert not any(key.startswith('MYCOPILOT_PDF_') for key in os.environ)
assert 'MYCOPILOT_INPUT_ROOT' not in os.environ
assert not any(key.startswith('MYCOPILOT_ARTIFACT_') for key in os.environ)
for label, operation in (
('fork', lambda: os.fork()),
('setsid', lambda: os.setsid()),
('setpgid', lambda: os.setpgid(0, 0)),
('exec-bash', lambda: os.execv('/bin/bash', ['bash', '-c', 'true'])),
('subprocess', lambda: subprocess.run(['/usr/bin/true'], check=True)),
('network', lambda: socket.create_connection(('127.0.0.1', 9), timeout=0.1)),
('system-passwd-read', lambda: Path('/etc/passwd').read_bytes()),
('system-version-read', lambda: Path('/System/Library/CoreServices/SystemVersion.plist').read_bytes()),
('external-read', lambda: Path({external:?}).read_bytes()),
('external-write', lambda: Path({external:?}).write_text('changed')),
('symlink', lambda: os.symlink({external:?}, 'outputs/escape-link')),
('hardlink', lambda: os.link({external:?}, 'outputs/escape-hardlink')),
):
try:
    operation()
    raise AssertionError(f'{{label}} sandbox escape unexpectedly succeeded')
except (PermissionError, OSError):
    pass

target = Path('outputs/smoke.pdf')
pdf = canvas.Canvas(str(target))
for page in range(40):
pdf.drawString(72, 720, f'Smoke marker page {{page}}')
pdf.showPage()
pdf.save()
assert len(PdfReader(str(target)).pages) == 40
with pdfplumber.open(target) as document:
assert 'Smoke marker page 0' in (document.pages[0].extract_text() or '')
pdfium.PdfDocument(target)[0].render(scale=1).to_pil().save('outputs/smoke.png')

form_source = Path('form-source.pdf')
form = canvas.Canvas(str(form_source))
form.drawString(72, 720, 'Founder')
form.acroForm.textfield(
name='founder', tooltip='Founder name', x=72, y=680, width=220, height=24
)
form.save()
source_reader = PdfReader(str(form_source))
assert source_reader.get_fields()['founder'].get('/V', '') == ''
filled = Path('outputs/form-filled.pdf')
writer = PdfWriter()
writer.clone_document_from_reader(source_reader)
writer.update_page_form_field_values(
writer.pages[0], {{'founder': 'Ada Lovelace'}}, auto_regenerate=False
)
with filled.open('wb') as stream:
writer.write(stream)
reopened = PdfReader(str(filled))
assert reopened.get_fields()['founder']['/V'] == 'Ada Lovelace'
widgets = [
annotation.get_object()
for annotation in reopened.pages[0].get('/Annots', [])
if annotation.get_object().get('/Subtype') == '/Widget'
]
assert widgets and widgets[0].get('/AP') is not None
pdfium.PdfDocument(filled)[0].render(scale=1).to_pil().save('outputs/form-filled.png')
flattened = Path('outputs/form-flattened.pdf')
flattened_writer = PdfWriter()
flattened_writer.clone_document_from_reader(reopened)
flattened_writer.update_page_form_field_values(
flattened_writer.pages[0], {{'founder': 'Ada Lovelace'}},
auto_regenerate=False, flatten=True
)
with flattened.open('wb') as stream:
flattened_writer.write(stream)
flattened_reader = PdfReader(str(flattened))
assert flattened_reader.pages[0].extract_text()
pdfium.PdfDocument(flattened)[0].render(scale=1).to_pil().save(
'outputs/form-flattened.png'
)
PY
pdfinfo outputs/smoke.pdf | rg --max-count 1 '^Pages:'
pdftotext outputs/smoke.pdf - | rg --max-count 1 'Smoke marker page 0'
"#,
        external = external.to_string_lossy(),
    );
    let run = |command: &str| {
        let shell_plan = super::super::managed_pdf_shell::parse_managed_pdf_shell(command)
            .unwrap()
            .expect("smoke command uses the managed PDF shell surface");
        let mut environment = managed_environment(&prepared.invocation, None);
        let launch = prepare_managed_pdf_shell_launch(
            &prepared.invocation,
            &shell_plan,
            &mut environment,
            &workspace,
            None,
            ManagedPdfToolPaths {
                runtime_root: provider.component_root(),
                pdf_cli: &provider.pdf_cli_path().unwrap(),
                ripgrep: &provider.ripgrep_executable().unwrap(),
            },
        )
        .unwrap();
        CommandSpawnPlan::direct(
            "managed PDF shell smoke".to_string(),
            workspace.execution_root().to_path_buf(),
            Some(workspace.execution_root()),
            Some(Duration::from_secs(60)),
            launch,
        )
        .build()
        .output()
        .unwrap()
    };
    let output = run(&script);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(workspace.outputs_root().join("smoke.pdf").is_file());
    assert!(workspace.outputs_root().join("smoke.png").is_file());
    assert!(workspace.outputs_root().join("form-filled.pdf").is_file());
    assert!(workspace.outputs_root().join("form-filled.png").is_file());
    assert!(workspace
        .outputs_root()
        .join("form-flattened.pdf")
        .is_file());
    assert!(workspace
        .outputs_root()
        .join("form-flattened.png")
        .is_file());

    let no_match = run("pdftotext outputs/smoke.pdf - | rg 'definitely-not-present'");
    assert!(
        !no_match.status.success(),
        "pipefail must retain rg failure"
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn managed_pdf_sandbox_fails_closed_on_unsupported_hosts() {
    let fixture = TempDir::new().unwrap();
    let workspace = ManagedCommandWorkspaceLease::open(fixture.path().join("managed-run")).unwrap();
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let runtime_root = executable.parent().unwrap().to_path_buf();
    let invocation = ArtifactRuntimeInvocation::new(
        "test".to_string(),
        "test".to_string(),
        "test".to_string(),
        ArtifactRuntimeKind::Python,
        "test".to_string(),
        executable,
        Vec::new(),
        BTreeMap::new(),
    );
    let shell_plan =
        super::super::managed_pdf_shell::parse_managed_pdf_shell("pdfinfo outputs/test.pdf")
            .unwrap()
            .unwrap();
    assert!(prepare_managed_pdf_shell_launch(
        &invocation,
        &shell_plan,
        &mut Vec::new(),
        &workspace,
        None,
        ManagedPdfToolPaths {
            runtime_root: &runtime_root,
            pdf_cli: &runtime_root,
            ripgrep: &runtime_root,
        },
    )
    .unwrap_err()
    .contains("拒绝裸进程执行"));
}

#[test]
fn managed_builder_output_contract_fails_closed_without_reaching_path() {
    let builder = infer_managed_artifact_builder_command(
        "python scripts/build.py --output 'outputs/report.docx'",
    )
    .unwrap()
    .unwrap();
    assert_eq!(builder.kind, AgentCommandRuntimeKind::Python);
    assert_eq!(builder.script, "scripts/build.py");
    assert_eq!(builder.output_paths, ["outputs/report.docx"]);

    for command in [
        "python scripts/build.py --output",
        "python scripts/build.py --output --title",
        "python scripts/build.py --output report.docx --output report.docx",
        "python scripts/build.py --output report.docx | tee build.log",
        "python scripts/build.py --output report.docx && echo done",
        "python -u scripts/build.py --output report.docx",
    ] {
        assert!(
            infer_managed_artifact_builder_command(command).is_err(),
            "malformed Builder intent unexpectedly fell through: {command}"
        );
    }
    assert!(
        infer_managed_artifact_builder_command("python ordinary.py | tee ordinary.log")
            .unwrap()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn builder_scope_rejects_existing_and_broken_external_symlinks() {
    use std::os::unix::fs::symlink;

    let workspace = TempDir::new().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    let outside = TempDir::new().unwrap();
    symlink(outside.path(), workspace_root.join("external")).unwrap();
    assert!(validate_managed_artifact_builder_output_scope(
        Some(&workspace_root),
        &workspace_root,
        &["external/report.docx".to_string()],
        AgentWritePermission::WorkspaceOnly,
    )
    .is_err());

    symlink(
        outside.path().join("missing"),
        workspace_root.join("broken"),
    )
    .unwrap();
    assert!(validate_managed_artifact_builder_output_scope(
        Some(&workspace_root),
        &workspace_root,
        &["broken/report.docx".to_string()],
        AgentWritePermission::WorkspaceOnly,
    )
    .is_err());
    assert!(validate_managed_artifact_builder_output_scope(
        Some(&workspace_root),
        &workspace_root,
        &["external/report.docx".to_string()],
        AgentWritePermission::All,
    )
    .is_ok());
}

#[test]
fn managed_environment_ignores_path_and_inherited_node_options() {
    let workspace = TempDir::new().unwrap();
    let fake_runtime = workspace.path().join("managed-node");
    let invocation = ArtifactRuntimeInvocation::new(
        "test-bundle".to_string(),
        "test-revision".to_string(),
        "test-fingerprint".to_string(),
        ArtifactRuntimeKind::Node,
        "test-node".to_string(),
        fake_runtime,
        Vec::new(),
        BTreeMap::new(),
    );
    let configured_environment = managed_environment(&invocation, None);
    assert!(!configured_environment
        .iter()
        .any(|(name, _)| name == OsStr::new("PATH")));
    assert!(!configured_environment
        .iter()
        .any(|(name, _)| name == OsStr::new("NODE_OPTIONS")));
}
