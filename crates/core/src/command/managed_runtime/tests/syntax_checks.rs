use super::*;

#[cfg(unix)]
fn fake_managed_node(runtime_root: &Path, marker: &Path) -> ArtifactRuntimeInvocation {
    use std::os::unix::fs::PermissionsExt;

    let executable = runtime_root.join("bin/node");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(
        &executable,
        br#"#!/bin/sh
if [ "$1" = "--check" ]; then
  script="$2"
  case "$script" in
/*) absolute_script="$script" ;;
*) absolute_script="$PWD/$script" ;;
  esac
  invalid=0
  while IFS= read -r line || [ -n "$line" ]; do
case "$line" in
  *SYNTAX_ERROR*) invalid=1 ;;
esac
  done < "$script"
  if [ "$invalid" -eq 1 ]; then
printf '%s: SyntaxError: fixture\n' "$absolute_script" >&2
printf 'runtime=%s\n' "$0" >&2
exit 1
  fi
  exit 0
fi
shift
output=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
shift
output="$1"
  fi
  shift
done
printf 'builder-ran' > "$TEST_BUILDER_MARKER"
if [ -n "$output" ]; then
  printf 'deck' > "$output"
fi
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    ArtifactRuntimeInvocation::new(
        "test-bundle".to_string(),
        "test-revision".to_string(),
        "test-fingerprint".to_string(),
        ArtifactRuntimeKind::Node,
        "test-node".to_string(),
        executable,
        Vec::new(),
        BTreeMap::from([(
            OsString::from("TEST_BUILDER_MARKER"),
            marker.as_os_str().to_os_string(),
        )]),
    )
}

#[cfg(unix)]
fn fake_managed_python(runtime_root: &Path, marker: &Path) -> ArtifactRuntimeInvocation {
    use std::os::unix::fs::PermissionsExt;

    let executable = runtime_root.join("bin/python3");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(
        &executable,
        br#"#!/bin/sh
if [ "$1" = "-I" ]; then shift; fi
if [ "$1" = "-B" ]; then shift; fi
if [ "$1" = "-c" ]; then
  shift
  check_program="$1"
  shift
  script="$1"
  case "$script" in
/*) absolute_script="$script" ;;
*) absolute_script="$PWD/$script" ;;
  esac
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
printf '  File "%s", line 1\nSyntaxError: fixture\n' "$absolute_script" >&2
printf 'runtime=%s\n' "$0" >&2
exit 1
  fi
  exit 0
fi
script="$1"
shift
output=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
shift
output="$1"
  fi
  shift
done
printf 'builder-ran' > "$TEST_BUILDER_MARKER"
if [ -n "$output" ]; then
  printf 'document' > "$output"
fi
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    ArtifactRuntimeInvocation::new(
        "test-bundle".to_string(),
        "test-revision".to_string(),
        "test-fingerprint".to_string(),
        ArtifactRuntimeKind::Python,
        "test-python".to_string(),
        executable,
        vec![OsString::from("-I"), OsString::from("-B")],
        BTreeMap::from([(
            OsString::from("TEST_BUILDER_MARKER"),
            marker.as_os_str().to_os_string(),
        )]),
    )
}

fn syntax_check_request(command: &str) -> AgentCommandRequest {
    AgentCommandRequest {
        id: "syntax-check-test".to_string(),
        command: command.to_string(),
        cwd: None,
        timeout_ms: None,
        approval_status: AgentApprovalStatus::Approved,
        risk_level: None,
        reason: None,
        observe: None,
        inputs: Vec::new(),
        runtime_binding: None,
        managed_office_script: None,
    }
}

fn syntax_check_resolution(
    profile: AgentCommandRuntimeProfile,
    kind: AgentCommandRuntimeKind,
) -> AgentCommandRuntimeResolution {
    AgentCommandRuntimeResolution {
        schema_version: AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
        provider_id: "test-provider".to_string(),
        profile: Some(profile),
        profile_revision: Some("test-profile".to_string()),
        bundle_version: Some("test-bundle".to_string()),
        bundle_revision: Some("test-revision".to_string()),
        kind,
        runtime_version: Some(
            match kind {
                AgentCommandRuntimeKind::Node => "test-node",
                AgentCommandRuntimeKind::Python => "test-python",
            }
            .to_string(),
        ),
        runtime_fingerprint: Some("test-fingerprint".to_string()),
        resolved_packages: Vec::new(),
        error_code: None,
        recovery: None,
        message: None,
    }
}

#[test]
fn validates_only_direct_saved_script_invocations() {
    assert!(parse_managed_artifact_command(
        "node scripts/build.mjs --output out.xlsx",
        AgentCommandRuntimeKind::Node,
    )
    .is_ok());
    for command in [
        "node -e console.log(1)",
        "node scripts/build.mjs | tee output.txt",
        "node scripts/build.mjs && echo done",
        "node $(printf script.mjs)",
        "python scripts/build.mjs",
    ] {
        assert!(
            parse_managed_artifact_command(command, AgentCommandRuntimeKind::Node).is_err(),
            "unexpectedly accepted {command}"
        );
    }
    assert!(parse_managed_artifact_command(
        "python3 scripts/build.py",
        AgentCommandRuntimeKind::Python,
    )
    .is_ok());
    assert!(
        parse_managed_artifact_command("python -m build", AgentCommandRuntimeKind::Python,)
            .is_err()
    );
}

#[test]
fn node_syntax_check_accepts_only_the_exact_saved_mjs_shape() {
    let parsed = parse_managed_artifact_command(
        "node --check 'scripts/build deck.mjs'",
        AgentCommandRuntimeKind::Node,
    )
    .unwrap();
    assert!(parsed.node_syntax_check);
    assert_eq!(parsed.script.as_deref(), Some("scripts/build deck.mjs"));
    assert_eq!(
        parsed.process_arguments,
        [
            OsString::from("--check"),
            OsString::from("scripts/build deck.mjs")
        ]
    );
    for command in [
        "node --check build.mjs --output deck.pptx",
        "node --check build.mjs | tee check.log",
        "node --check build.mjs && node build.mjs",
        "node --check -- build.mjs",
    ] {
        assert!(
            infer_managed_artifact_builder_command(command).is_err(),
            "malformed syntax check unexpectedly passed: {command}"
        );
    }
    for ordinary in ["node --check", "node --check build.js"] {
        assert!(
            infer_managed_artifact_builder_command(ordinary)
                .unwrap()
                .is_none(),
            "ordinary Node check was incorrectly claimed as a managed .mjs Builder: {ordinary}"
        );
    }
}

#[cfg(unix)]
#[test]
fn automatic_node_builder_syntax_gate_fails_closed_without_running_builder() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("private-workspace");
    let runtime_root = fixture.path().join("private-runtime");
    fs::create_dir_all(&workspace).unwrap();
    let marker = workspace.join("builder.marker");
    let expected_output = workspace.join("deck.pptx");
    let script = "build_deck.mjs";
    fs::write(workspace.join(script), "const broken = SYNTAX_ERROR;\n").unwrap();
    let invocation = fake_managed_node(&runtime_root, &marker);
    let environment = managed_environment(&invocation, None);
    let execution = run_managed_builder_syntax_check(
        &invocation,
        script,
        &workspace,
        &environment,
        &AgentCancellationToken::new(),
        managed_builder_syntax_check_redactions(
            &runtime_root,
            &invocation,
            &workspace,
            script,
            ManagedBuilderSyntaxLanguage::Node,
        ),
        ManagedBuilderSyntaxLanguage::Node,
    );
    assert!(!execution.succeeded());
    assert_eq!(execution.exit_code, Some(1));
    assert!(
        !marker.exists(),
        "syntax failure must not execute Builder body"
    );
    assert!(
        !expected_output.exists(),
        "syntax failure must not create the declared output"
    );

    let result = managed_builder_syntax_check_failure_result(
        Some(&workspace),
        &workspace,
        &syntax_check_request("node build_deck.mjs --output deck.pptx"),
        syntax_check_resolution(
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Node,
        ),
        execution,
        ManagedBuilderSyntaxLanguage::Node,
    );
    assert_eq!(
        result.runtime.as_ref().unwrap().error_code.as_deref(),
        Some(ERROR_BUILDER_SYNTAX_INVALID)
    );
    assert!(result.error.as_deref().unwrap().contains("Builder 未启动"));
    for private in [
        workspace.to_string_lossy().as_ref(),
        runtime_root.to_string_lossy().as_ref(),
        invocation.executable().to_string_lossy().as_ref(),
    ] {
        assert!(!result.stderr.contains(private));
    }
    assert!(result.stderr.contains(script));
    let mut spool = String::new();
    result
        .stderr_spool
        .reopen()
        .unwrap()
        .read_to_string(&mut spool)
        .unwrap();
    assert!(!spool.contains(workspace.to_string_lossy().as_ref()));
    assert!(!spool.contains(runtime_root.to_string_lossy().as_ref()));
    assert!(spool.contains(script));
}

#[cfg(unix)]
#[test]
fn valid_node_builder_syntax_gate_allows_the_pinned_builder_launch() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    let runtime_root = fixture.path().join("runtime");
    fs::create_dir_all(&workspace).unwrap();
    let marker = workspace.join("builder.marker");
    let output = workspace.join("deck.pptx");
    let script = "build_deck.mjs";
    fs::write(workspace.join(script), "export const deck = true;\n").unwrap();
    let invocation = fake_managed_node(&runtime_root, &marker);
    let environment = managed_environment(&invocation, None);
    let syntax_check = run_managed_builder_syntax_check(
        &invocation,
        script,
        &workspace,
        &environment,
        &AgentCancellationToken::new(),
        managed_builder_syntax_check_redactions(
            &runtime_root,
            &invocation,
            &workspace,
            script,
            ManagedBuilderSyntaxLanguage::Node,
        ),
        ManagedBuilderSyntaxLanguage::Node,
    );
    assert!(syntax_check.succeeded());
    assert!(
        !marker.exists(),
        "syntax check itself must not run Builder body"
    );

    let launch = CommandDirectLaunchPlan::isolated(
        invocation.executable().to_path_buf(),
        [
            OsString::from(script),
            OsString::from("--output"),
            output.as_os_str().to_os_string(),
        ]
        .into_iter()
        .collect(),
        environment,
    );
    let status = CommandSpawnPlan::direct(
        "node build_deck.mjs --output deck.pptx".to_string(),
        workspace.clone(),
        Some(&workspace),
        Some(Duration::from_secs(5)),
        launch,
    )
    .build()
    .status()
    .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(marker).unwrap(), "builder-ran");
    assert_eq!(fs::read_to_string(output).unwrap(), "deck");
}

#[test]
fn automatic_builder_syntax_gate_is_scoped_to_managed_office_output_commands() {
    let python_parsed = parse_managed_artifact_command(
        "python build.py --output report.docx",
        AgentCommandRuntimeKind::Python,
    )
    .unwrap();
    let python_builder =
        infer_managed_artifact_builder_command("python build.py --output report.docx")
            .unwrap()
            .unwrap();
    for profile in [
        AgentCommandRuntimeProfile::Documents,
        AgentCommandRuntimeProfile::Spreadsheets,
    ] {
        assert_eq!(
            automatic_managed_builder_syntax_check(
                profile,
                AgentCommandRuntimeKind::Python,
                &python_parsed,
                Some(&python_builder),
            ),
            Some(ManagedBuilderSyntaxLanguage::Python)
        );
    }
    assert_eq!(
        automatic_managed_builder_syntax_check(
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Python,
            &python_parsed,
            Some(&python_builder),
        ),
        None
    );

    let check_only =
        parse_managed_artifact_command("node --check build.mjs", AgentCommandRuntimeKind::Node)
            .unwrap();
    let check_builder = infer_managed_artifact_builder_command("node --check build.mjs")
        .unwrap()
        .unwrap();
    assert_eq!(
        automatic_managed_builder_syntax_check(
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Node,
            &check_only,
            Some(&check_builder),
        ),
        None,
        "an explicit Node syntax check must not recursively trigger the automatic gate"
    );
}

#[cfg(unix)]
#[test]
fn automatic_python_builder_syntax_gate_fails_closed_without_running_builder() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("private-workspace");
    let runtime_root = fixture.path().join("private-runtime");
    fs::create_dir_all(&workspace).unwrap();
    let marker = workspace.join("builder.marker");
    let output = workspace.join("report.docx");
    let script = "build_document.py";
    fs::write(workspace.join(script), "PYTHON_SYNTAX_ERROR\n").unwrap();
    let invocation = fake_managed_python(&runtime_root, &marker);
    let environment = managed_environment(&invocation, None);
    let execution = run_managed_builder_syntax_check(
        &invocation,
        script,
        &workspace,
        &environment,
        &AgentCancellationToken::new(),
        managed_builder_syntax_check_redactions(
            &runtime_root,
            &invocation,
            &workspace,
            script,
            ManagedBuilderSyntaxLanguage::Python,
        ),
        ManagedBuilderSyntaxLanguage::Python,
    );
    assert!(!execution.succeeded());
    assert_eq!(execution.exit_code, Some(1));
    assert!(!marker.exists(), "syntax failure must not run the Builder");
    assert!(!output.exists(), "syntax failure must not create output");

    let result = managed_builder_syntax_check_failure_result(
        Some(&workspace),
        &workspace,
        &syntax_check_request("python build_document.py --output report.docx"),
        syntax_check_resolution(
            AgentCommandRuntimeProfile::Documents,
            AgentCommandRuntimeKind::Python,
        ),
        execution,
        ManagedBuilderSyntaxLanguage::Python,
    );
    assert_eq!(
        result.runtime.as_ref().unwrap().error_code.as_deref(),
        Some(ERROR_BUILDER_SYNTAX_INVALID)
    );
    assert!(result.error.as_deref().unwrap().contains("Python"));
    for private in [
        workspace.to_string_lossy().as_ref(),
        runtime_root.to_string_lossy().as_ref(),
        invocation.executable().to_string_lossy().as_ref(),
    ] {
        assert!(!result.stderr.contains(private));
    }
    assert!(result.stderr.contains(script));
}

#[cfg(unix)]
#[test]
fn valid_python_builder_syntax_gate_allows_the_pinned_builder_launch() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    let runtime_root = fixture.path().join("runtime");
    fs::create_dir_all(&workspace).unwrap();
    let marker = workspace.join("builder.marker");
    let output = workspace.join("report.docx");
    let script = "build_document.py";
    fs::write(workspace.join(script), "title = 'valid'\n").unwrap();
    let invocation = fake_managed_python(&runtime_root, &marker);
    let environment = managed_environment(&invocation, None);
    let syntax_check = run_managed_builder_syntax_check(
        &invocation,
        script,
        &workspace,
        &environment,
        &AgentCancellationToken::new(),
        managed_builder_syntax_check_redactions(
            &runtime_root,
            &invocation,
            &workspace,
            script,
            ManagedBuilderSyntaxLanguage::Python,
        ),
        ManagedBuilderSyntaxLanguage::Python,
    );
    assert!(syntax_check.succeeded());
    assert!(
        !marker.exists(),
        "syntax check itself must not run the Builder"
    );

    let mut arguments = invocation.arguments_prefix().to_vec();
    arguments.extend([
        OsString::from(script),
        OsString::from("--output"),
        output.as_os_str().to_os_string(),
    ]);
    let status = CommandSpawnPlan::direct(
        "python build_document.py --output report.docx".to_string(),
        workspace.clone(),
        Some(&workspace),
        Some(Duration::from_secs(5)),
        CommandDirectLaunchPlan::isolated(
            invocation.executable().to_path_buf(),
            arguments,
            environment,
        ),
    )
    .build()
    .status()
    .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(marker).unwrap(), "builder-ran");
    assert_eq!(fs::read_to_string(output).unwrap(), "document");
}

#[cfg(unix)]
#[test]
fn managed_session_preparation_blocks_invalid_python_before_builder_launch() {
    use std::io::Write;

    let workspace = TempDir::new().unwrap();
    fs::write(
        workspace.path().join("build_document.py"),
        "PYTHON_SYNTAX_ERROR\n",
    )
    .unwrap();
    let output_path = workspace.path().join("report.docx");
    let file = fs::File::create(&output_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", options).unwrap();
    archive
        .write_all(
            b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>",
        )
        .unwrap();
    archive.start_file("word/document.xml", options).unwrap();
    archive.write_all(b"<document>original</document>").unwrap();
    archive.finish().unwrap();
    let original_output = fs::read(&output_path).unwrap();

    let (_runtime, provider) = create_test_artifact_runtime();
    let binding = super::super::prepare_command_runtime_profile(
        &provider,
        AgentCommandRuntimeProfile::Documents,
        AgentCommandRuntimeKind::Python,
    )
    .unwrap()
    .binding;
    let request = AgentCommandRequest {
        id: "python-syntax-preflight-integration".to_string(),
        command: "python build_document.py --output report.docx".to_string(),
        cwd: None,
        timeout_ms: Some(5_000),
        approval_status: AgentApprovalStatus::Approved,
        risk_level: None,
        reason: Some("Verify Python syntax before document generation".to_string()),
        observe: Some(AgentCommandArtifactObservationRequest {
            kinds: vec![AgentCommandArtifactObservationKind::Office],
            expected_outputs: vec!["report.docx".to_string()],
            additional_roots: Vec::new(),
        }),
        inputs: Vec::new(),
        runtime_binding: Some(Box::new(binding)),
        managed_office_script: None,
    };
    let input_context = AgentFileInputExecutionContext::default();
    let preparation = prepare_managed_command_session(
        Some(workspace.path()),
        &request,
        editor_permissions(),
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        ManagedCommandSessionServices {
            artifact_runtime: Some(provider),
            office_engine: None,
            file_inputs: Some(&input_context),
            managed_workspace: None,
        },
    )
    .unwrap();
    let ManagedCommandSessionPreparation::Immediate(result) = preparation else {
        panic!("invalid Python must fail during preparation before a Builder process exists")
    };

    assert_eq!(
        result.runtime.as_ref().unwrap().error_code.as_deref(),
        Some(ERROR_BUILDER_SYNTAX_INVALID)
    );
    assert_eq!(fs::read(&output_path).unwrap(), original_output);
    assert!(!workspace.path().join("managed-builder-ran.marker").exists());
    let observation = result
        .artifact_observation
        .as_ref()
        .expect("syntax failure retains authoritative output observation");
    assert_eq!(observation.expected_outputs.len(), 1);
    assert_eq!(
        observation.expected_outputs[0].outcome,
        crate::AgentCommandExpectedArtifactOutcomeKind::Unchanged
    );
}
