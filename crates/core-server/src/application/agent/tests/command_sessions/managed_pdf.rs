use super::*;

#[cfg(target_os = "macos")]
#[test]
#[ignore = "manual Aspen PDF smoke; requires MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT and MYCOPILOT_ASPEN_PDF"]
fn aspen_pdf_runs_a_five_step_managed_session_workflow() {
    let component = std::env::var_os("MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT")
        .expect("set MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT");
    let source_aspen = std::path::PathBuf::from(
        std::env::var_os("MYCOPILOT_ASPEN_PDF").expect("set MYCOPILOT_ASPEN_PDF"),
    )
    .canonicalize()
    .expect("canonical Aspen PDF path");
    let file_name = source_aspen
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .expect("Aspen PDF file name is UTF-8")
        .to_string();
    let bytes = std::fs::read(&source_aspen).expect("read Aspen PDF fixture");
    let fixture =
        RunningFixture::new_with_initial_yield("aspen-five-step-smoke", Duration::from_secs(120));
    let workspace = fixture.workspace.path();
    std::fs::write(workspace.join(&file_name), &bytes)
        .expect("copy Aspen PDF into smoke workspace");
    let input = mycopilot_core::AgentFileInputBinding {
        schema_version: mycopilot_core::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
        mount_path: "aspen.pdf".to_string(),
        source: mycopilot_core::AgentFileInputRef::Workspace { path: file_name },
        size_bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    let provider = Arc::new(
        ArtifactRuntimeProvider::discover(
            &ArtifactRuntimeDiscoveryOptions::new().with_configured_component_dir(component),
        )
        .expect("discover prepared Artifact Runtime"),
    );
    let binding = provider
        .resolve_profile(
            mycopilot_core::AgentCommandRuntimeProfile::Pdf,
            mycopilot_core::AgentCommandRuntimeKind::Python,
        )
        .expect("resolve frozen PDF Runtime profile");
    let file_inputs = mycopilot_core::file_input::AgentFileInputExecutionContext::default();

    let execute = |suffix: &str,
                   command: &str,
                   inputs: &[mycopilot_core::AgentFileInputBinding]|
     -> mycopilot_core::command::AgentCommandExecutionResult {
        let call_id = format!("{}-{suffix}", fixture.call_id);
        let request = AgentCommandRequest {
            id: call_id.clone(),
            command: command.to_string(),
            cwd: None,
            timeout_ms: None,
            approval_status: AgentApprovalStatus::Approved,
            risk_level: Some(AgentCommandRiskLevel::ReadOnly),
            reason: Some("Aspen managed PDF release smoke".to_string()),
            observe: None,
            inputs: inputs.to_vec(),
            runtime_binding: Some(Box::new(binding.clone())),
            managed_office_script: None,
        };
        let tracker = Arc::new(FileEffectTracker::default());
        let mut file_effect_guard = Some(tracker.register(
            None,
            Some(&fixture.conversation_id),
            &fixture.run_id,
            &call_id,
        ));
        file_effect_guard.as_mut().unwrap().mark_effects_started();
        let launch = fixture
            .registry
            .start(StartAgentCommandSession {
                owner: CommandSessionOwner {
                    conversation_id: fixture.conversation_id.clone(),
                    assistant_message_id: fixture.assistant_message_id.clone(),
                    origin_run_id: fixture.run_id.clone(),
                    call_id,
                    project_id: None,
                },
                workspace_root: Some(workspace),
                command: &request,
                permissions: AgentPermissions {
                    read: mycopilot_core::AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
                authorization_source: CommandAuthorizationSource::ExplicitUser,
                approval_provenance: json!({
                    "source": "explicit_user",
                    "status": "approved",
                    "manualSmoke": true
                }),
                artifact_runtime: Some(Arc::clone(&provider)),
                office_engine: None,
                file_inputs: Some(&file_inputs),
                notifications: None,
                cancellation_token: AgentCancellationToken::new(),
                cancel_probe: None,
                file_effect_guard: &mut file_effect_guard,
            })
            .expect("start managed Aspen command");
        let AgentCommandSessionLaunch::Exited(terminal) = launch else {
            panic!("Aspen smoke step exceeded the 120 second synchronous test yield")
        };
        terminal.execution
    };

    let metadata = execute(
        "metadata",
        "pdfinfo \"$MYCOPILOT_INPUT_ROOT/aspen.pdf\"",
        std::slice::from_ref(&input),
    );
    assert_eq!(metadata.exit_code, Some(0), "stderr={}", metadata.stderr);
    assert!(metadata.stdout.contains("Pages:"));

    let search = execute(
        "search",
        "pdftotext -layout \"$MYCOPILOT_INPUT_ROOT/aspen.pdf\" - | rg -n -i -C 4 --max-count 20 'steady-state unit operation|RStoic|RCSTR'",
        std::slice::from_ref(&input),
    );
    assert_eq!(search.exit_code, Some(0), "stderr={}", search.stderr);
    assert!(search.stdout.contains("RStoic"));
    assert!(search.stdout.contains("RCSTR"));

    let locate = execute(
        "locate",
        "pdftotext -f 306 -l 314 -layout \"$MYCOPILOT_INPUT_ROOT/aspen.pdf\" - | rg -n -i -C 3 --max-count 40 'Dupl|Flash2|Heater|Mixer|RStoic|RCSTR'",
        std::slice::from_ref(&input),
    );
    assert_eq!(
        locate.exit_code,
        Some(0),
        "stderr={} error={:?} runtime={:?} timedOut={} cancelled={}",
        locate.stderr,
        locate.error,
        locate.runtime,
        locate.timed_out,
        locate.cancelled
    );
    for expected in ["Dupl", "Flash2", "Heater", "Mixer", "RStoic", "RCSTR"] {
        assert!(
            locate.stdout.contains(expected),
            "missing {expected} from located Aspen unit models: {}",
            locate.stdout
        );
    }

    let render = execute(
        "render",
        "pdftoppm -f 307 -l 308 -r 96 -png \"$MYCOPILOT_INPUT_ROOT/aspen.pdf\" outputs/aspen-steady",
        std::slice::from_ref(&input),
    );
    assert_eq!(render.exit_code, Some(0), "stderr={}", render.stderr);
    assert_eq!(render.outputs.len(), 2);
    assert!(render.outputs.iter().all(|output| {
        serde_json::to_value(output.kind).unwrap() == json!("image")
            && output.read_path.starts_with("image-artifact://sha256/")
    }));

    for output in &render.outputs {
        let digest = output
            .read_path
            .strip_prefix("image-artifact://sha256/")
            .expect("published page exposes an image Artifact read path");
        let artifact = fixture
            .storage
            .read_authorized_managed_artifact(&format!("sha256:{digest}"), &fixture.conversation_id)
            .unwrap()
            .expect("published page remains readable by its conversation");
        assert_eq!(artifact.sha256, digest);
        assert_eq!(artifact.media_type, "image/png");
        assert!(!artifact.bytes.is_empty());
    }

    let private_path = fixture
        .workspace
        .path()
        .join("managed-command-runs")
        .to_string_lossy()
        .into_owned();
    for execution in [&metadata, &search, &locate, &render] {
        let projected = serde_json::to_string(&mycopilot_core::command::command_tool_result(
            "aspen-smoke",
            execution,
        ))
        .unwrap();
        assert!(!projected.contains(&private_path));
        assert!(!projected.contains("managed-command-runs"));
    }
    let sessions = fixture
        .storage
        .list_agent_command_sessions(&fixture.conversation_id, 16)
        .unwrap();
    assert_eq!(sessions.len(), 4);
    assert!(sessions
        .iter()
        .any(|record| record.snapshot.outputs.len() == 2));

    // Freeze one additional approval input, mutate it before the Host execution boundary, and
    // prove the same production Session path fails closed before spawning a sixth process.
    let changed_path = workspace.join("approval-race.pdf");
    std::fs::write(&changed_path, &bytes).unwrap();
    let changed_input = mycopilot_core::AgentFileInputBinding {
        schema_version: mycopilot_core::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
        mount_path: "approval-race.pdf".to_string(),
        source: mycopilot_core::AgentFileInputRef::Workspace {
            path: "approval-race.pdf".to_string(),
        },
        size_bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    std::fs::write(&changed_path, b"%PDF-1.7\nchanged after approval\n%%EOF\n").unwrap();
    let changed = execute(
        "changed-input",
        "pdfinfo \"$MYCOPILOT_INPUT_ROOT/approval-race.pdf\"",
        std::slice::from_ref(&changed_input),
    );
    assert_eq!(changed.exit_code, None);
    assert_eq!(
        changed
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.error_code.as_deref()),
        Some("agent.fileInput.integrityMismatch")
    );
    assert_eq!(
        fixture
            .storage
            .list_agent_command_sessions(&fixture.conversation_id, 16)
            .unwrap()
            .len(),
        4,
        "changed approval input must fail before creating a process Session"
    );
}
