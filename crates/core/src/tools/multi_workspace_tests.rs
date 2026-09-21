use super::{AgentTool, ToolExecutionContext, ToolRegistry};
use crate::file_change::{FileChangeDirectoryIdentity, FileChangePathPolicy};
use crate::storage::models::ProjectFolderRole;
use crate::workspace::WorkspaceFolder;
use crate::{
    AgentApprovalStatus, AgentAttachmentLibraryContext, AgentFolderReference, AgentPermissions,
    AgentProposedAction, AgentRunContext, AgentToolCall, AgentWorkspaceContext,
};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

struct Workspace {
    _temp: tempfile::TempDir,
    main: PathBuf,
    docs: PathBuf,
    workspace: AgentWorkspaceContext,
}

impl Workspace {
    fn new() -> Self {
        Self::in_temp(tempfile::tempdir().unwrap())
    }

    fn in_temp(temp: tempfile::TempDir) -> Self {
        let main = temp.path().join("main");
        let docs = temp.path().join("docs");
        fs::create_dir(&main).unwrap();
        fs::create_dir(&docs).unwrap();
        let main = main.canonicalize().unwrap();
        let docs = docs.canonicalize().unwrap();
        let folder = |path: &Path, alias: &str, role| WorkspaceFolder {
            id: format!("folder-{alias}"),
            alias: alias.to_string(),
            role,
            path: path.to_string_lossy().into_owned(),
            canonical_path: Some(path.to_string_lossy().into_owned()),
            directory_identity: Some(FileChangeDirectoryIdentity::read(path).unwrap()),
        };
        let workspace = AgentWorkspaceContext {
            project_id: Some("project-multi".to_string()),
            display_name: Some("multi".to_string()),
            root_path: Some(main.to_string_lossy().into_owned()),
            folders: vec![
                folder(&main, "main", ProjectFolderRole::Primary),
                folder(&docs, "docs", ProjectFolderRole::Auxiliary),
            ],
        };
        Self {
            _temp: temp,
            main,
            docs,
            workspace,
        }
    }

    fn context(&self) -> ToolExecutionContext {
        self.context_with_permissions(AgentPermissions {
            write: crate::AgentWritePermission::WorkspaceOnly,
            ..AgentPermissions::default()
        })
    }

    fn context_with_permissions(&self, permissions: AgentPermissions) -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("conversation-multi".to_string()),
            project_id: Some("project-multi".to_string()),
            workspace: Some(self.workspace.clone()),
            permissions,
            attachment_library: None,
            collaboration_identity: None,
        }))
        .with_runtime_services("run-multi".to_string(), None)
        .with_tool_call_id("multi-fixture-call".to_string())
    }

    fn read(&self, context: &ToolExecutionContext, path: &str, call_id: &str) -> Value {
        super::read_file::ReadFileTool
            .execute(
                &context.clone().with_tool_call_id(call_id.to_string()),
                json!({"path": path}),
            )
            .unwrap()
    }

    fn folder_context(&self) -> ToolExecutionContext {
        self.folder_context_with_permissions(AgentPermissions::default())
    }

    fn folder_context_with_permissions(
        &self,
        permissions: AgentPermissions,
    ) -> ToolExecutionContext {
        let reference = AgentFolderReference::new("picked", "docs")
            .unwrap()
            .with_root_path(self.docs.to_string_lossy().into_owned());
        let attachment_library = AgentAttachmentLibraryContext {
            root_path: None,
            conversation_id: Some("conversation-multi".to_string()),
            project_id: Some("project-multi".to_string()),
            conversation_attachments: Vec::new(),
            project_attachments: Vec::new(),
            folder_references: vec![reference],
        };
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("conversation-multi".to_string()),
            project_id: Some("project-multi".to_string()),
            workspace: None,
            permissions,
            attachment_library: Some(attachment_library),
            collaboration_identity: None,
        }))
        .with_runtime_services("run-multi".to_string(), None)
        .with_tool_call_id("folder-fixture-call".to_string())
    }
}

#[test]
fn multi_workspace_reads_same_name_and_literal_namespace_without_aliasing() {
    let fixture = Workspace::new();
    fs::write(fixture.main.join("README.md"), "primary\n").unwrap();
    fs::write(fixture.docs.join("README.md"), "auxiliary\n").unwrap();
    fs::create_dir_all(fixture.main.join("@workspace/docs")).unwrap();
    fs::write(fixture.main.join("@workspace/docs/README.md"), "literal\n").unwrap();
    let context = fixture.context();
    for (path, expected) in [
        ("README.md", "primary\n"),
        ("@workspace/docs/README.md", "auxiliary\n"),
        ("./@workspace/docs/README.md", "literal\n"),
    ] {
        let result = fixture.read(&context, path, path);
        assert_eq!(result["content"], expected);
        assert_eq!(result["path"], path);
        assert_eq!(result["fileChangeTarget"]["filePath"], path);
    }
    assert!(
        context
            .resolve_existing_path(fixture.docs.join("README.md").to_str().unwrap())
            .is_err(),
        "Read workspace-only must retain its absolute-path policy"
    );
}

#[test]
fn multi_workspace_explicit_search_and_map_preserve_namespace_and_default_to_primary() {
    let fixture = Workspace::new();
    fs::write(fixture.main.join("README.md"), "primary needle\n").unwrap();
    fs::write(fixture.docs.join("README.md"), "auxiliary needle\n").unwrap();
    let context = fixture.context();
    let files = super::search_files::SearchFilesTool
        .execute(&context, json!({"query":"README"}))
        .unwrap();
    assert!(files.to_string().contains("README.md"));
    assert!(!files.to_string().contains("@workspace/"));
    let files = super::search_files::SearchFilesTool
        .execute(
            &context,
            json!({"query":"README", "path":"@workspace/docs"}),
        )
        .unwrap();
    assert!(files.to_string().contains("@workspace/docs/README.md"));
    let code = super::search_code::SearchCodeTool
        .execute(
            &context,
            json!({"query":"needle", "path":"@workspace/docs"}),
        )
        .unwrap();
    assert!(code.to_string().contains("@workspace/docs/README.md"));
    assert!(code.to_string().contains("auxiliary needle"));
    let map = super::workspace_map::WorkspaceMapTool
        .execute(&context, json!({"focusPath":"@workspace/docs"}))
        .unwrap();
    assert_eq!(map["tree"][0]["path"], "@workspace/docs/README.md");
    assert!(map["summary"]
        .to_string()
        .contains("@workspace/docs/README.md"));
    let map = super::workspace_map::WorkspaceMapTool
        .execute(&context, json!({}))
        .unwrap();
    assert_eq!(map["tree"][0]["path"], "README.md");
}

#[test]
fn selected_folder_is_readable_and_listed_without_all_path_permission() {
    let fixture = Workspace::new();
    fs::create_dir(fixture.docs.join("src")).unwrap();
    fs::write(fixture.docs.join("src/main.rs"), "fn main() {}\n").unwrap();
    let context = fixture.folder_context();
    let root_path = fixture.docs.to_string_lossy().to_string();
    let file_path = fixture
        .docs
        .join("src/main.rs")
        .to_string_lossy()
        .to_string();

    let read = fixture.read(&context, file_path.as_str(), "folder-read");
    assert_eq!(read["content"], "fn main() {}\n");
    assert_eq!(read["path"], file_path.as_str());

    let files = super::search_files::SearchFilesTool
        .execute(&context, json!({"query":"main", "path":root_path.as_str()}))
        .unwrap();
    assert_eq!(files["matches"][0]["path"], file_path.as_str());

    let map = super::workspace_map::WorkspaceMapTool
        .execute(&context, json!({"focusPath":root_path.as_str()}))
        .unwrap();
    assert_eq!(map["workspace"]["focusPath"], root_path.as_str());
    assert!(map["tree"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == file_path.as_str()));

    // The default permission fixture denies writes, so a missing path cannot be planned here.
    assert!(context
        .resolve_missing_file_observation_target(fixture.docs.join("src/new.rs").to_str().unwrap())
        .is_err());
}

#[test]
fn selected_folder_absolute_paths_follow_global_write_permission() {
    let fixture = Workspace::new();
    fs::write(fixture.docs.join("README.md"), "auxiliary\n").unwrap();
    let context = fixture.folder_context_with_permissions(AgentPermissions {
        read: crate::AgentReadPermission::All,
        write: crate::AgentWritePermission::All,
        ..AgentPermissions::default()
    });
    let path = fixture
        .docs
        .join("README.md")
        .to_string_lossy()
        .into_owned();
    let new_path = fixture.docs.join("new.rs").to_string_lossy().into_owned();

    assert!(context.is_selected_folder_path(&path));
    assert!(context
        .resolve_missing_file_observation_target(&new_path)
        .is_ok());

    let call = AgentToolCall {
        id: "selected-folder-write".to_string(),
        tool: "apply_patch".to_string(),
        args: json!({
            "request": {
                "action": "apply",
                "operation": "create",
                "filePath": new_path,
                "content": "created\n"
            }
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let AgentProposedAction::FileChange { file_change } = ToolRegistry::defaults_with_search(None)
        .proposed_action(&context, &call)
        .unwrap()
    else {
        panic!("expected FileChange")
    };
    assert_eq!(file_change.file_path, new_path);

    let workspace_only = fixture.folder_context_with_permissions(AgentPermissions {
        write: crate::AgentWritePermission::WorkspaceOnly,
        ..AgentPermissions::default()
    });
    assert!(workspace_only
        .resolve_missing_file_observation_target(&new_path)
        .is_err());
}

#[test]
fn multi_workspace_missing_or_replaced_auxiliary_never_disables_primary_or_rebinds_auxiliary() {
    let fixture = Workspace::new();
    fs::write(fixture.main.join("README.md"), "primary\n").unwrap();
    let context = fixture.context();
    fs::rename(&fixture.docs, fixture.docs.with_file_name("old-docs")).unwrap();
    assert_eq!(
        fixture.read(&context, "README.md", "read-primary")["content"],
        "primary\n"
    );
    assert!(context.resolve_existing_path("@workspace/docs").is_err());
    fs::create_dir(&fixture.docs).unwrap();
    fs::write(fixture.docs.join("README.md"), "replacement\n").unwrap();
    assert!(context
        .resolve_existing_path("@workspace/docs/README.md")
        .is_err());
    assert!(
        FileChangePathPolicy::from_workspace(Some(&fixture.workspace), true)
            .resolve("@workspace/docs/new.md")
            .is_err()
    );
    for invalid in [
        "@workspace/unknown/README.md",
        "@workspace/docs/../main/README.md",
    ] {
        assert!(context.resolve_existing_path(invalid).is_err());
    }
}

#[test]
fn multi_workspace_apply_patch_uses_exact_auxiliary_observation_and_display_path() {
    let fixture = Workspace::new();
    fs::write(fixture.main.join("README.md"), "primary\n").unwrap();
    fs::write(fixture.docs.join("README.md"), "auxiliary\n").unwrap();
    let context = fixture.context();
    let observed = fixture.read(&context, "@workspace/docs/README.md", "read-aux");
    let registry = ToolRegistry::defaults_with_search(None);
    let request = |path: &str| AgentToolCall {
        id: format!("update-{path}"),
        tool: "apply_patch".to_string(),
        args: json!({"request":{"action":"apply", "operation":"update", "filePath":path, "observationId":observed["observationId"], "content":"updated\n"}}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    assert!(registry
        .proposed_action(&context, &request("README.md"))
        .is_err());
    let AgentProposedAction::FileChange { file_change } = registry
        .proposed_action(&context, &request("@workspace/docs/README.md"))
        .unwrap()
    else {
        panic!("expected FileChange")
    };
    assert_eq!(file_change.file_path, "@workspace/docs/README.md");
    assert_eq!(
        Path::new(&file_change.execution.canonical_target),
        fixture.docs.join("README.md")
    );
    assert_eq!(
        file_change.execution.base_content.as_deref(),
        Some("auxiliary\n")
    );
    file_change.execution.validate().unwrap();
}

#[test]
fn multi_workspace_file_change_revalidates_root_even_if_target_parent_is_preserved() {
    let fixture = Workspace::new();
    fs::create_dir(fixture.docs.join("nested")).unwrap();
    let target = FileChangePathPolicy::from_workspace(Some(&fixture.workspace), false)
        .resolve("@workspace/docs/nested/new.txt")
        .unwrap();
    let old = fixture.docs.with_file_name("old-docs");
    fs::rename(&fixture.docs, &old).unwrap();
    fs::create_dir(&fixture.docs).unwrap();
    fs::rename(old.join("nested"), fixture.docs.join("nested")).unwrap();
    assert!(target.revalidate().is_err());
}

#[cfg(unix)]
#[test]
fn multi_workspace_file_change_preserves_symlink_and_hardlink_rejection() {
    use std::os::unix::fs::symlink;
    let fixture = Workspace::new();
    let policy = FileChangePathPolicy::from_workspace(Some(&fixture.workspace), false);
    fs::write(fixture.main.join("target.txt"), "primary\n").unwrap();
    symlink(
        fixture.main.join("target.txt"),
        fixture.docs.join("link.txt"),
    )
    .unwrap();
    fs::hard_link(
        fixture.main.join("target.txt"),
        fixture.docs.join("hard.txt"),
    )
    .unwrap();
    assert!(policy.resolve("@workspace/docs/link.txt").is_err());
    assert!(policy.resolve("@workspace/docs/hard.txt").is_err());
    let target = policy.resolve("@workspace/docs/new.txt").unwrap();
    assert_eq!(target.display_path(), "@workspace/docs/new.txt");
}

#[cfg(unix)]
#[test]
fn multi_workspace_read_all_absolute_rejects_retargeted_root_but_allows_external_files() {
    use std::os::unix::fs::symlink;
    let fixture = Workspace::new();
    let outside = tempfile::tempdir().unwrap();
    let external_file = outside.path().join("external.txt");
    fs::write(&external_file, "external\n").unwrap();
    let context = fixture.context_with_permissions(AgentPermissions {
        read: crate::AgentReadPermission::All,
        write: crate::AgentWritePermission::All,
        ..AgentPermissions::default()
    });
    fs::rename(&fixture.docs, fixture.docs.with_file_name("original-docs")).unwrap();
    symlink(outside.path(), &fixture.docs).unwrap();
    let retargeted = fixture
        .docs
        .join("external.txt")
        .to_string_lossy()
        .into_owned();
    assert!(context.resolve_existing_path(&retargeted).is_err());
    assert!(context
        .resolve_existing_path_preserving_leaf(&retargeted)
        .is_err());
    assert!(super::read_file::ReadFileTool
        .execute(&context, json!({"path":retargeted}))
        .is_err());
    let external_path = external_file.to_string_lossy();
    assert_eq!(
        fixture.read(&context, &external_path, "external-read")["content"],
        "external\n"
    );
}

#[cfg(unix)]
#[test]
fn multi_workspace_read_all_system_alias_rejects_retargeted_declared_root() {
    use std::os::unix::fs::symlink;
    let home = dirs::home_dir().unwrap();
    let fixture = Workspace::in_temp(tempfile::tempdir_in(&home).unwrap());
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("external.txt"), "external\n").unwrap();
    let context = fixture.context_with_permissions(AgentPermissions {
        read: crate::AgentReadPermission::All,
        ..AgentPermissions::default()
    });
    let relative = fixture
        .docs
        .strip_prefix(home.canonicalize().unwrap())
        .unwrap();
    let alias = format!("@home/{}/external.txt", relative.to_string_lossy());
    fs::rename(&fixture.docs, fixture.docs.with_file_name("original-docs")).unwrap();
    symlink(outside.path(), &fixture.docs).unwrap();
    assert!(context.resolve_existing_path(&alias).is_err());
    assert!(context
        .resolve_existing_path_preserving_leaf(&alias)
        .is_err());
    assert!(context
        .display_path(&alias, &outside.path().join("external.txt"))
        .is_err());
}
