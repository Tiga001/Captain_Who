use super::*;

fn frozen_workspace(primary: &Path, auxiliary: &Path) -> crate::AgentWorkspaceContext {
    use crate::storage::models::{ProjectFolderRecord, ProjectFolderRole, ProjectRecord};
    let mut project =
        ProjectRecord::with_primary_folder("project", "Project", primary.to_string_lossy(), 0);
    project.folders[0].alias = "app".into();
    project.folders.push(ProjectFolderRecord {
        id: "folder-docs".into(),
        alias: "docs".into(),
        path: auxiliary.to_string_lossy().into_owned(),
        role: ProjectFolderRole::Auxiliary,
        sort_order: 1,
        created_at: 0,
    });
    crate::workspace::freeze_project_workspace(&project).unwrap()
}

#[test]
fn multi_workspace_managed_session_executes_auxiliary_script_with_frozen_input_and_cwd() {
    let primary = TempDir::new().unwrap();
    let auxiliary = TempDir::new().unwrap();
    fs::write(auxiliary.path().join("task.py"), "# fixture script").unwrap();
    fs::write(auxiliary.path().join("input.txt"), "auxiliary input").unwrap();
    let workspace = frozen_workspace(primary.path(), auxiliary.path());
    let inputs = AgentFileInputExecutionContext::default().with_workspace(Some(&workspace));
    let (_runtime, provider) = create_test_artifact_runtime_with_python(
        br#"#!/bin/sh
/bin/cat "$MYCOPILOT_INPUT_ROOT/input.txt"
printf 'a,b\n1,2\n' > report.csv
"#
        .to_vec(),
    );
    let binding = super::super::prepare_command_runtime_profile(
        &provider,
        AgentCommandRuntimeProfile::Documents,
        AgentCommandRuntimeKind::Python,
    )
    .unwrap()
    .binding;
    let permissions = editor_permissions();
    let request = AgentCommandRequest {
        id: "auxiliary-managed".into(),
        command: "python task.py".into(),
        cwd: Some("@workspace/docs".into()),
        timeout_ms: Some(5000),
        approval_status: AgentApprovalStatus::Approved,
        risk_level: None,
        reason: None,
        observe: Some(AgentCommandArtifactObservationRequest {
            kinds: vec![AgentCommandArtifactObservationKind::Office],
            expected_outputs: vec!["@workspace/docs/report.csv".into()],
            additional_roots: vec!["@workspace/docs".into()],
        }),
        inputs: prepare_agent_file_input_bindings(
            Some(primary.path()),
            permissions,
            &inputs,
            &[AgentFileInputSpec {
                mount_path: "input.txt".into(),
                source: AgentFileInputRef::Workspace {
                    path: "@workspace/docs/input.txt".into(),
                },
            }],
            None,
        )
        .unwrap(),
        runtime_binding: Some(Box::new(binding)),
        managed_office_script: None,
    };
    let manager = crate::command::CommandSessionManager::new(
        crate::command::CommandSessionManagerConfig::default(),
    )
    .unwrap();
    let outcome = manager
        .start_authorized_command_with_runtime_and_lifecycle(
            crate::command::CommandSessionScopeId::new("multi-workspace-managed").unwrap(),
            Some(primary.path()),
            &request,
            permissions,
            CommandAuthorizationSource::ExplicitUser,
            crate::command::CommandStartOptions {
                initial_yield: Duration::from_millis(1),
            },
            Some(provider),
            None,
            Some(&inputs),
            None,
            Arc::new(|_| {}),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    let terminal = match outcome {
        crate::command::CommandStartOutcome::Exited(result) => *result,
        crate::command::CommandStartOutcome::Running(snapshot) => manager
            .wait_terminal_result(&snapshot.session_id, Duration::from_secs(5))
            .unwrap()
            .unwrap(),
    };
    assert_eq!(
        terminal.execution.exit_code,
        Some(0),
        "{:?}",
        terminal.execution
    );
    assert_eq!(terminal.execution.stdout, "auxiliary input");
    assert_eq!(terminal.execution.cwd, "@workspace/docs");
    assert!(auxiliary.path().join("report.csv").is_file());
    assert!(!primary.path().join("report.csv").exists());
    assert!(terminal
        .execution
        .artifact_observation
        .unwrap()
        .changes
        .iter()
        .any(|change| change.path == "@workspace/docs/report.csv"));
}

#[test]
fn multi_workspace_managed_runtime_rejects_user_writable_runtime_and_private_cwd_escape() {
    let primary = TempDir::new().unwrap();
    fs::write(primary.path().join("task.py"), "# fixture script").unwrap();
    let (runtime, provider) = create_test_artifact_runtime();
    let workspace = frozen_workspace(primary.path(), runtime.path());
    let inputs = AgentFileInputExecutionContext::default().with_workspace(Some(&workspace));
    let binding = super::super::prepare_command_runtime_profile(
        &provider,
        AgentCommandRuntimeProfile::Documents,
        AgentCommandRuntimeKind::Python,
    )
    .unwrap()
    .binding;
    let mut request = AgentCommandRequest {
        id: "runtime-authority".into(),
        command: "python task.py".into(),
        cwd: None,
        timeout_ms: Some(5000),
        approval_status: AgentApprovalStatus::Approved,
        risk_level: None,
        reason: None,
        observe: None,
        inputs: vec![],
        runtime_binding: Some(Box::new(binding)),
        managed_office_script: None,
    };
    let result = prepare_managed_command_session(
        Some(primary.path()),
        &request,
        editor_permissions(),
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        ManagedCommandSessionServices {
            artifact_runtime: Some(provider.clone()),
            office_engine: None,
            file_inputs: Some(&inputs),
            managed_workspace: None,
        },
    );
    assert!(result.is_err());
    // A writable root nested inside runtime dependencies is unsafe even though the runtime's
    // component root and launcher themselves are outside that writable folder.
    let nested_workspace = frozen_workspace(primary.path(), &runtime.path().join("dependencies"));
    let nested_inputs =
        AgentFileInputExecutionContext::default().with_workspace(Some(&nested_workspace));
    let result = prepare_managed_command_session(
        Some(primary.path()),
        &request,
        editor_permissions(),
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        ManagedCommandSessionServices {
            artifact_runtime: Some(provider.clone()),
            office_engine: None,
            file_inputs: Some(&nested_inputs),
            managed_workspace: None,
        },
    );
    assert!(result.is_err());
    request.runtime_binding.as_mut().unwrap().profile = AgentCommandRuntimeProfile::Pdf;
    request.cwd = Some("@workspace/docs".into());
    let private = TempDir::new().unwrap();
    let result = prepare_managed_command_session(
        Some(primary.path()),
        &request,
        editor_permissions(),
        CommandAuthorizationSource::ExplicitUser,
        AgentCancellationToken::new(),
        ManagedCommandSessionServices {
            artifact_runtime: Some(provider),
            office_engine: None,
            file_inputs: Some(&inputs),
            managed_workspace: Some(
                ManagedCommandWorkspaceLease::open(private.path().join("execution")).unwrap(),
            ),
        },
    );
    assert!(result.is_err());
}
