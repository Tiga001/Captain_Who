pub(super) fn frozen_file_change_recovery_workspace(
    root: &std::path::Path,
) -> mycopilot_core::AgentWorkspaceContext {
    let root = root.canonicalize().unwrap();
    let path = root.to_string_lossy().into_owned();
    mycopilot_core::AgentWorkspaceContext {
        project_id: None,
        display_name: Some("FileChange recovery fixture".to_string()),
        root_path: Some(path.clone()),
        folders: vec![mycopilot_core::workspace::WorkspaceFolder {
            id: "file-change-recovery-primary".to_string(),
            alias: "main".to_string(),
            role: mycopilot_core::storage::models::ProjectFolderRole::Primary,
            path: path.clone(),
            canonical_path: Some(path),
            directory_identity: Some(
                mycopilot_core::file_change::FileChangeDirectoryIdentity::read(&root).unwrap(),
            ),
        }],
    }
}
