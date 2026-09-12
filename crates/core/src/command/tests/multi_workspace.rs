use super::*;
use crate::file_input::{agent_file_input_ref_from_model_path, read_verified_agent_file_input};
use crate::storage::models::{ProjectFolderRecord, ProjectFolderRole, ProjectRecord};
use crate::workspace::{freeze_project_workspace, WorkspaceResolver};

fn workspace(primary: &Path, auxiliary: &Path) -> crate::AgentWorkspaceContext {
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
    freeze_project_workspace(&project).unwrap()
}

#[test]
fn multi_workspace_file_inputs_preserve_namespace_and_revalidate_bytes() {
    let primary = TestWorkspace::new();
    let auxiliary = TestWorkspace::new();
    fs::write(primary.path.join("shared.txt"), "primary").unwrap();
    fs::write(auxiliary.path.join("shared.txt"), "auxiliary").unwrap();
    let frozen = workspace(&primary.path, &auxiliary.path);
    let inputs = AgentFileInputExecutionContext::default().with_workspace(Some(&frozen));
    let permissions = AgentPermissions::default();
    let source =
        agent_file_input_ref_from_model_path(&inputs, "@workspace/docs/shared.txt").unwrap();
    let snapshot = read_verified_agent_file_input(
        Some(&primary.path),
        permissions,
        &inputs,
        &source,
        None,
        1024,
    )
    .unwrap();
    assert_eq!(snapshot.bytes, b"auxiliary");
    let absolute = agent_file_input_ref_from_model_path(
        &inputs,
        &auxiliary.path.join("shared.txt").to_string_lossy(),
    )
    .unwrap();
    let absolute_snapshot = read_verified_agent_file_input(
        Some(&primary.path),
        permissions,
        &inputs,
        &absolute,
        None,
        1024,
    )
    .unwrap();
    assert_eq!(absolute_snapshot.source, source);
    let bindings = prepare_agent_file_input_bindings(
        Some(&primary.path),
        permissions,
        &inputs,
        &[crate::AgentFileInputSpec {
            mount_path: "shared.txt".into(),
            source,
        }],
        None,
    )
    .unwrap();
    fs::write(auxiliary.path.join("shared.txt"), "changed").unwrap();
    assert!(materialize_agent_file_inputs(
        Some(&primary.path),
        permissions,
        &inputs,
        &bindings,
        None
    )
    .is_err());
    let unknown = crate::AgentFileInputRef::Workspace {
        path: "@workspace/missing/shared.txt".into(),
    };
    assert!(read_verified_agent_file_input(
        Some(&primary.path),
        permissions,
        &inputs,
        &unknown,
        None,
        1024
    )
    .is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            primary.path.join("shared.txt"),
            auxiliary.path.join("link.txt"),
        )
        .unwrap();
        let linked = crate::AgentFileInputRef::Workspace {
            path: "@workspace/docs/link.txt".into(),
        };
        assert!(read_verified_agent_file_input(
            Some(&primary.path),
            permissions,
            &inputs,
            &linked,
            None,
            1024
        )
        .is_err());
    }
}

#[test]
fn multi_workspace_command_cwd_and_managed_output_scope_share_frozen_membership() {
    let primary = TestWorkspace::new();
    let auxiliary = TestWorkspace::new();
    let outside = TestWorkspace::new();
    let frozen = workspace(&primary.path, &auxiliary.path);
    let resolver = WorkspaceResolver::from_context(Some(&frozen));
    assert_eq!(
        resolve_command_cwd_in_workspace(&resolver, None, AgentWritePermission::WorkspaceOnly)
            .unwrap(),
        primary.path
    );
    assert_eq!(
        resolve_command_cwd_in_workspace(
            &resolver,
            Some("@workspace/docs"),
            AgentWritePermission::WorkspaceOnly
        )
        .unwrap(),
        auxiliary.path
    );
    assert!(resolve_command_cwd_in_workspace(
        &resolver,
        Some("@workspace/docs/../app"),
        AgentWritePermission::All
    )
    .is_err());
    assert!(resolve_command_cwd_in_workspace(
        &resolver,
        Some(&outside.path.to_string_lossy()),
        AgentWritePermission::WorkspaceOnly
    )
    .is_err());
    validate_managed_artifact_builder_output_scope_in_workspace(
        &resolver,
        &auxiliary.path,
        &["new.docx".into()],
        AgentWritePermission::WorkspaceOnly,
    )
    .unwrap();
    assert!(validate_managed_artifact_builder_output_scope_in_workspace(
        &resolver,
        &auxiliary.path,
        &[outside.path.join("new.docx").to_string_lossy().into_owned()],
        AgentWritePermission::WorkspaceOnly
    )
    .is_err());
    let literal = primary.path.join("@workspace/docs");
    fs::create_dir_all(&literal).unwrap();
    assert_eq!(
        resolve_command_cwd_in_workspace(
            &resolver,
            Some("./@workspace/docs"),
            AgentWritePermission::WorkspaceOnly
        )
        .unwrap(),
        literal
    );
    let moved = auxiliary.path.with_extension("previous");
    fs::rename(&auxiliary.path, &moved).unwrap();
    fs::create_dir(&auxiliary.path).unwrap();
    assert!(resolve_command_cwd_in_workspace(
        &resolver,
        Some("@workspace/docs"),
        AgentWritePermission::All
    )
    .is_err());
    fs::remove_dir_all(&moved).unwrap();
}

#[test]
fn multi_workspace_background_command_preserves_auxiliary_cwd_inputs_and_observation() {
    let primary = TestWorkspace::new();
    let auxiliary = TestWorkspace::new();
    fs::write(primary.path.join("marker.txt"), "primary").unwrap();
    fs::write(auxiliary.path.join("marker.txt"), "auxiliary").unwrap();
    let frozen = workspace(&primary.path, &auxiliary.path);
    let inputs = AgentFileInputExecutionContext::default().with_workspace(Some(&frozen));
    let permissions = AgentPermissions {
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        ..AgentPermissions::default()
    };
    let mut command = request(
        "sleep 0.05; printf 'a,b\\n1,2\\n' > report.csv; cat marker.txt",
        Some(5000),
    );
    command.cwd = Some("@workspace/docs".into());
    command.inputs = prepare_agent_file_input_bindings(
        Some(&primary.path),
        permissions,
        &inputs,
        &[crate::AgentFileInputSpec {
            mount_path: "shared.txt".into(),
            source: crate::AgentFileInputRef::Workspace {
                path: "@workspace/docs/marker.txt".into(),
            },
        }],
        None,
    )
    .unwrap();
    command.observe = Some(AgentCommandArtifactObservationRequest {
        kinds: vec![AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["@workspace/docs/report.csv".into()],
        additional_roots: vec!["@workspace/docs".into()],
    });
    let manager = CommandSessionManager::new(CommandSessionManagerConfig::default()).unwrap();
    let outcome = manager
        .start_authorized_command_with_runtime_and_lifecycle(
            CommandSessionScopeId::new("multi-workspace").unwrap(),
            Some(&primary.path),
            &command,
            permissions,
            CommandAuthorizationSource::ExplicitUser,
            CommandStartOptions {
                initial_yield: Duration::from_millis(1),
            },
            None,
            None,
            Some(&inputs),
            None,
            Arc::new(|_| {}),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    let terminal = match outcome {
        CommandStartOutcome::Exited(terminal) => *terminal,
        CommandStartOutcome::Running(snapshot) => manager
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
    assert_eq!(terminal.execution.stdout, "auxiliary");
    assert_eq!(terminal.execution.cwd, "@workspace/docs");
    assert!(!primary.path.join("report.csv").exists());
    assert!(auxiliary.path.join("report.csv").is_file());
    let observation = terminal.execution.artifact_observation.unwrap();
    assert!(observation
        .changes
        .iter()
        .any(|change| change.path == "@workspace/docs/report.csv"
            && change.scope == AgentCommandArtifactScope::Workspace));
    assert!(observation
        .expected_outputs
        .iter()
        .any(
            |output| output.path.as_deref() == Some("@workspace/docs/report.csv")
                && output.scope == Some(AgentCommandArtifactScope::Workspace)
        ));
}
