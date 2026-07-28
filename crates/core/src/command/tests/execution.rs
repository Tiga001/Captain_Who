use super::*;

#[test]
fn runs_simple_command_in_workspace() {
    let workspace = TestWorkspace::new();
    let result = run_authorized_command(
        Some(&workspace.path),
        &request("printf hello", Some(5_000)),
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            ..Default::default()
        },
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout, "hello");
    assert_eq!(result.cwd, ".");
    assert!(!result.cancelled);
}

#[test]
fn compatibility_entry_never_executes_a_host_bound_runtime_through_path() {
    let workspace = TestWorkspace::new();
    let marker = workspace.path.join("compatibility-bypass.txt");
    let mut command = request(
        "printf compatibility-bypass > compatibility-bypass.txt",
        Some(5_000),
    );
    command.runtime_binding = Some(Box::new(frozen_runtime_binding()));

    let result = run_authorized_command(
        Some(&workspace.path),
        &command,
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            ..Default::default()
        },
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert!(!marker.exists());
    assert!(result.runtime.is_some());
    assert!(result
        .error
        .as_deref()
        .is_some_and(|error| error.contains("Managed Artifact Runtime")));
}

#[cfg(unix)]
#[test]
fn managed_builder_output_scope_is_revalidated_before_spawn() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new();
    let outside = TestWorkspace::new();
    symlink(&outside.path, workspace.path.join("exports")).unwrap();
    let mut command = request("python build.py --output exports/report.docx", Some(5_000));
    command.runtime_binding = Some(Box::new(frozen_runtime_binding()));

    let error = run_authorized_command(
        Some(&workspace.path),
        &command,
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            ..Default::default()
        },
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap_err();

    assert!(error.to_string().contains("当前权限只允许写入 workspace"));
    assert!(!outside.path.join("report.docx").exists());
}

#[test]
fn runs_simple_command_outside_workspace_with_full_write() {
    let workspace = TestWorkspace::new();
    let outside = TestWorkspace::new();
    let mut request = request("printf outside", Some(5_000));
    request.cwd = Some(outside.path.to_string_lossy().to_string());

    let result = run_authorized_command(
        Some(&workspace.path),
        &request,
        AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            ..Default::default()
        },
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout, "outside");
    assert_eq!(result.cwd, outside.path.to_string_lossy());
}

#[test]
fn executor_rechecks_workspace_only_read_scope_against_canonical_cwd() {
    let workspace = TestWorkspace::new();
    let outside = TestWorkspace::new();
    std::fs::write(outside.path.join("local.txt"), "outside-secret").unwrap();
    let mut command = request("cat local.txt", Some(5_000));
    command.cwd = Some(outside.path.to_string_lossy().to_string());
    let permissions = AgentPermissions {
        read: AgentReadPermission::WorkspaceOnly,
        write: AgentWritePermission::All,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        ..Default::default()
    };

    let error = run_authorized_command(
        Some(&workspace.path),
        &command,
        permissions,
        CommandAuthorizationSource::Automatic,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap_err();
    assert_eq!(
        error
            .policy_evaluation()
            .map(|evaluation| evaluation.decision),
        Some(CommandPolicyDecision::RequireExplicitApproval)
    );
    assert!(error
        .policy_evaluation()
        .is_some_and(|evaluation| evaluation
            .findings
            .iter()
            .any(|finding| finding.code == "command.scope.external_cwd")));

    let result = run_authorized_command(
        Some(&workspace.path),
        &command,
        permissions,
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();
    assert_eq!(result.stdout, "outside-secret");
}

#[test]
fn times_out_long_command() {
    let workspace = TestWorkspace::new();
    let result = run_authorized_command(
        Some(&workspace.path),
        &request("sleep 2", Some(50)),
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            ..Default::default()
        },
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert!(result.timed_out);
}

#[test]
fn pre_start_action_cancellation_never_spawns_the_command() {
    let workspace = TestWorkspace::new();
    let cancel_flag = Arc::new(AtomicBool::new(true));
    let result = run_shell_command(
        &workspace.path,
        Some(&workspace.path),
        &request("mkdir must-not-exist", Some(5_000)),
        AgentCancellationToken::new(),
        Some(cancel_flag),
    )
    .unwrap();

    assert!(result.cancelled);
    assert_eq!(result.exit_code, None);
    assert!(!workspace.path.join("must-not-exist").exists());
}

#[cfg(unix)]
#[test]
fn timeout_terminates_the_shell_process_group() {
    let workspace = TestWorkspace::new();
    let started = Instant::now();
    let result = run_shell_command(
        &workspace.path,
        Some(&workspace.path),
        &request("sleep 30 & wait", Some(50)),
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert!(result.timed_out);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn completed_shell_does_not_leave_a_descendant_holding_output_pipes() {
    let workspace = TestWorkspace::new();
    let started = Instant::now();
    let result = run_shell_command(
        &workspace.path,
        Some(&workspace.path),
        &request("sleep 30 &", Some(5_000)),
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn drains_and_bounds_large_stdout_and_stderr_without_deadlock() {
    let workspace = TestWorkspace::new();
    let result = run_shell_command(
        &workspace.path,
        Some(&workspace.path),
        &request(
            "head -c 200000 /dev/zero; head -c 200000 /dev/zero >&2",
            Some(5_000),
        ),
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(result.stdout_truncated);
    assert!(result.stderr_truncated);
    assert!(result.stdout.len() <= MAX_OUTPUT_BYTES);
    assert!(result.stderr.len() <= MAX_OUTPUT_BYTES);
    assert_eq!(result.output_capture.original_bytes, 400_000);
    assert_eq!(result.output_capture.captured_bytes, 400_000);
    assert_eq!(result.output_capture.omitted_bytes, 0);
    assert!(!result.output_capture.truncated_at_source);
    assert!(result.output_capture.stdout_preview_truncated);
    assert!(result.output_capture.stderr_preview_truncated);
    assert_eq!(result.output_spool_substitutions().len(), 2);
    assert_eq!(
        result
            .stdout_spool
            .reopen()
            .unwrap()
            .metadata()
            .unwrap()
            .len(),
        200_000
    );
    assert_eq!(
        result
            .stderr_spool
            .reopen()
            .unwrap()
            .metadata()
            .unwrap()
            .len(),
        200_000
    );
    let tool_result = command_tool_result("call-large-output", &result);
    assert_eq!(
        tool_result.result.as_ref().unwrap()["truncatedAtSource"],
        false
    );
    assert_eq!(
        tool_result.result.as_ref().unwrap()["originalBytes"],
        400_000
    );
    let exact = tool_result
        .exact_archive_file
        .as_ref()
        .expect("complete process output should have an exact archive sidecar");
    let archived: serde_json::Value = serde_json::from_reader(exact.reopen().unwrap()).unwrap();
    assert_eq!(
        archived["result"]["stdout"].as_str().unwrap().len(),
        200_000
    );
    assert_eq!(
        archived["result"]["stderr"].as_str().unwrap().len(),
        200_000
    );
}

#[test]
fn execution_recomputes_policy_instead_of_trusting_declared_risk() {
    let workspace = TestWorkspace::new();
    let mut command = request("printf trusted", Some(5_000));
    command.risk_level = Some(AgentCommandRiskLevel::Destructive);

    let result = run_authorized_command(
        Some(&workspace.path),
        &command,
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            ..Default::default()
        },
        CommandAuthorizationSource::Automatic,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert_eq!(result.stdout, "trusted");

    let mut disguised_catastrophe = request("rm -rf /", Some(5_000));
    disguised_catastrophe.risk_level = Some(AgentCommandRiskLevel::ReadOnly);
    let error = run_authorized_command(
        Some(&workspace.path),
        &disguised_catastrophe,
        AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            ..Default::default()
        },
        CommandAuthorizationSource::Automatic,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("command.catastrophic.filesystem_root"));
    assert_eq!(
        error
            .policy_evaluation()
            .map(|evaluation| evaluation.code.as_str()),
        Some("command.catastrophic.filesystem_root")
    );
}

#[test]
fn executor_refuses_guarded_automatic_command_that_needs_approval() {
    let workspace = TestWorkspace::new();
    let error = run_authorized_command(
        Some(&workspace.path),
        &request("printf blocked > output.txt", Some(5_000)),
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            ..Default::default()
        },
        CommandAuthorizationSource::Automatic,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("command.explicit_approval_required"));
    assert_eq!(
        error
            .policy_evaluation()
            .map(|evaluation| evaluation.decision),
        Some(CommandPolicyDecision::RequireExplicitApproval)
    );
    assert!(!workspace.path.join("output.txt").exists());
}

fn observe_office_output(request: &mut AgentCommandRequest, path: &str) {
    request.observe = Some(AgentCommandArtifactObservationRequest {
        kinds: vec![AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec![path.to_string()],
        additional_roots: Vec::new(),
    });
}

fn explicit_workspace_permissions() -> AgentPermissions {
    AgentPermissions {
        read: AgentReadPermission::WorkspaceOnly,
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::RequireApproval,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        ..Default::default()
    }
}

#[test]
fn nonzero_exit_preserves_created_office_artifact_observation() {
    let workspace = TestWorkspace::new();
    let mut command = request("printf failed-output > failed.csv; false", Some(5_000));
    observe_office_output(&mut command, "failed.csv");

    let result = run_authorized_command(
        Some(&workspace.path),
        &command,
        explicit_workspace_permissions(),
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert_eq!(result.exit_code, Some(1));
    let observation = result.artifact_observation.unwrap();
    assert_eq!(
        observation.expected_outputs[0].outcome,
        AgentCommandExpectedArtifactOutcomeKind::Created
    );
    assert!(observation.changes.iter().any(|change| {
        change.path == "failed.csv" && change.kind == AgentCommandArtifactChangeKind::Created
    }));
}

#[test]
fn timeout_preserves_artifacts_created_before_process_termination() {
    let workspace = TestWorkspace::new();
    let mut command = request("printf timed-output > timed.csv; sleep 30", Some(50));
    observe_office_output(&mut command, "timed.csv");

    let result = run_authorized_command(
        Some(&workspace.path),
        &command,
        explicit_workspace_permissions(),
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        None,
    )
    .unwrap();

    assert!(result.timed_out);
    let observation = result.artifact_observation.unwrap();
    assert_eq!(
        observation.expected_outputs[0].outcome,
        AgentCommandExpectedArtifactOutcomeKind::Created
    );
    assert!(observation
        .changes
        .iter()
        .any(|change| change.path == "timed.csv"));
}

#[test]
fn cancellation_preserves_artifacts_created_before_process_termination() {
    let workspace = TestWorkspace::new();
    let mut command = request(
        "printf cancelled-output > cancelled.csv; sleep 30",
        Some(10_000),
    );
    observe_office_output(&mut command, "cancelled.csv");
    let cancellation = AgentCancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let workspace_path = workspace.path.clone();
    let worker = std::thread::spawn(move || {
        run_authorized_command(
            Some(&workspace_path),
            &command,
            explicit_workspace_permissions(),
            CommandAuthorizationSource::ExplicitUser,
            worker_cancellation,
            None,
        )
        .unwrap()
    });
    let output = workspace.path.join("cancelled.csv");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !output.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        output.exists(),
        "command did not create its output before cancellation"
    );
    cancellation.cancel();
    let result = worker.join().unwrap();

    assert!(result.cancelled);
    let observation = result.artifact_observation.unwrap();
    assert_eq!(
        observation.expected_outputs[0].outcome,
        AgentCommandExpectedArtifactOutcomeKind::Created
    );
    assert!(observation
        .changes
        .iter()
        .any(|change| change.path == "cancelled.csv"));
}
