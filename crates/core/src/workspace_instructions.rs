//! Workspace instruction discovery (`AGENTS.md`) for the model-visible World State.
//!
//! Each frozen workspace folder root contributes at most one file: `AGENTS.override.md` when it
//! exists, otherwise `AGENTS.md`. Files are concatenated in a stable order (primary folder first,
//! then by alias) and capped at [`MAX_WORKSPACE_INSTRUCTIONS_BYTES`]. Discovery is read-only and
//! shared by the Host request path and context-window previews, so both sides always agree on the
//! conventions the next request would use.
//!
//! Discovery is also bounded and safe: unavailable folders, missing or empty files and read errors
//! are skipped without failing the run. Every read revalidates the frozen directory identity and
//! opens candidates without following a leaf symlink, so a symlinked instruction file or a root
//! replaced by a symlink can never redirect instruction content into the model context from
//! outside the captured workspace.
use crate::storage::models::ProjectFolderRole;
use crate::workspace::{WorkspaceFolder, WorkspaceResolver};
use crate::world_state::WorkspaceInstructionSource;
use crate::AgentRunContext;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::Path;

/// Total instruction budget across all workspace folders, applied at read time.
pub const MAX_WORKSPACE_INSTRUCTIONS_BYTES: usize = 32 * 1024;

const CANDIDATE_FILENAMES: [&str; 2] = ["AGENTS.override.md", "AGENTS.md"];

#[derive(Debug, Clone)]
pub struct LoadedWorkspaceInstructions {
    pub sources: Vec<WorkspaceInstructionSource>,
    pub truncated: bool,
}

/// Reads the instruction file of every available workspace folder. `None` means no folder
/// contributes instructions and the section should stay absent.
pub fn load_workspace_instructions(
    context: Option<&AgentRunContext>,
) -> Option<LoadedWorkspaceInstructions> {
    let workspace = context?.workspace.as_ref()?;
    let resolver = WorkspaceResolver::from_context(Some(workspace));
    let mut folders: Vec<&WorkspaceFolder> = workspace.folders.iter().collect();
    // Stable, UI-independent order keeps World State revisions free of display-order noise.
    folders.sort_by_key(|folder| {
        let rank = if folder.role == ProjectFolderRole::Primary {
            0u8
        } else {
            1u8
        };
        (rank, folder.alias.clone())
    });
    let mut sources = Vec::new();
    let mut remaining = MAX_WORKSPACE_INSTRUCTIONS_BYTES;
    let mut truncated = false;
    'folders: for folder in folders {
        // Revalidate the frozen directory identity before every read. A root that was replaced
        // after the freeze (including by a symlink pointing elsewhere) must never redirect
        // discovery, so the cached canonical path is never opened directly.
        let Ok(root) = resolver.validated_root(folder) else {
            continue;
        };
        for name in CANDIDATE_FILENAMES {
            let Ok(file) = open_regular_file_no_follow(&root.join(name)) else {
                continue;
            };
            let Ok(metadata) = file.metadata() else {
                break;
            };
            if !metadata.is_file() {
                continue;
            }
            if metadata.len() == 0 {
                // The first existing candidate wins; an empty file intentionally silences the
                // folder, matching the tools that stop at the first matching path.
                break;
            }
            if remaining == 0 {
                // More instruction content exists than the budget allows; report the truncation.
                truncated = true;
                break 'folders;
            }
            let mut data = Vec::new();
            if file.take(remaining as u64).read_to_end(&mut data).is_err() {
                break;
            }
            // Revalidate once more after reading: a directory swap racing the read must discard
            // the captured bytes instead of publishing them.
            if resolver.validated_root(folder).is_err() {
                break;
            }
            if metadata.len() as usize > remaining {
                truncated = true;
            }
            let content = String::from_utf8_lossy(&data).into_owned();
            if !content.trim().is_empty() {
                remaining -= data.len();
                sources.push(WorkspaceInstructionSource {
                    folder_alias: folder.alias.clone(),
                    relative_path: name.to_string(),
                    content,
                });
            }
            break;
        }
    }
    if sources.is_empty() {
        None
    } else {
        Some(LoadedWorkspaceInstructions { sources, truncated })
    }
}

/// Opens a candidate without following a leaf symlink, mirroring the repository's other
/// no-follow readers. On Unix `O_NOFOLLOW` rejects a symlinked leaf; the surrounding identity
/// checks reject replaced roots on every platform.
fn open_regular_file_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_change::FileChangeDirectoryIdentity;
    use crate::{AgentPermissions, AgentWorkspaceContext};
    use std::fs;
    use tempfile::tempdir;

    fn folder(root: &Path, alias: &str, role: ProjectFolderRole) -> WorkspaceFolder {
        let canonical = fs::canonicalize(root).unwrap();
        let canonical = canonical.to_string_lossy().into_owned();
        WorkspaceFolder {
            id: format!("{alias}-folder"),
            alias: alias.to_string(),
            role,
            path: root.to_string_lossy().into_owned(),
            canonical_path: Some(canonical.clone()),
            directory_identity: Some(
                FileChangeDirectoryIdentity::read(Path::new(&canonical)).unwrap(),
            ),
        }
    }

    fn context(folders: Vec<WorkspaceFolder>) -> AgentRunContext {
        // `validate_shape` requires `root_path` to mirror the primary folder.
        let root_path = folders
            .iter()
            .find(|folder| folder.role == ProjectFolderRole::Primary)
            .or_else(|| folders.first())
            .map(|folder| folder.path.clone());
        AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-instructions".into()),
            project_id: Some("project-instructions".into()),
            workspace: Some(AgentWorkspaceContext {
                project_id: Some("project-instructions".into()),
                display_name: Some("Instructions".into()),
                root_path,
                folders,
            }),
            attachment_library: None,
            permissions: AgentPermissions::default(),
        }
    }

    #[test]
    fn reads_override_before_agents_and_orders_primary_first() {
        let aux = tempdir().unwrap();
        let primary = tempdir().unwrap();
        fs::write(aux.path().join("AGENTS.md"), "aux base").unwrap();
        fs::write(aux.path().join("AGENTS.override.md"), "aux override").unwrap();
        fs::write(primary.path().join("AGENTS.md"), "primary base").unwrap();
        let loaded = load_workspace_instructions(Some(&context(vec![
            folder(aux.path(), "zeta", ProjectFolderRole::Auxiliary),
            folder(primary.path(), "beta", ProjectFolderRole::Primary),
        ])))
        .expect("both folders contribute instructions");
        assert!(!loaded.truncated);
        let reads = loaded
            .sources
            .iter()
            .map(|source| {
                (
                    source.folder_alias.as_str(),
                    source.relative_path.as_str(),
                    source.content.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reads,
            vec![
                ("beta", "AGENTS.md", "primary base"),
                ("zeta", "AGENTS.override.md", "aux override"),
            ]
        );
    }

    #[test]
    fn skips_missing_empty_unavailable_and_blank_folders() {
        let missing = tempdir().unwrap();
        let empty = tempdir().unwrap();
        let blank = tempdir().unwrap();
        let unavailable = tempdir().unwrap();
        fs::write(empty.path().join("AGENTS.md"), "").unwrap();
        fs::write(blank.path().join("AGENTS.override.md"), "  \n").unwrap();
        fs::write(unavailable.path().join("AGENTS.md"), "never read").unwrap();
        let mut unavailable_folder =
            folder(unavailable.path(), "ghost", ProjectFolderRole::Auxiliary);
        unavailable_folder.canonical_path = None;
        assert!(load_workspace_instructions(Some(&context(vec![
            folder(missing.path(), "missing", ProjectFolderRole::Primary),
            folder(empty.path(), "empty", ProjectFolderRole::Auxiliary),
            folder(blank.path(), "blank", ProjectFolderRole::Auxiliary),
            unavailable_folder,
        ])))
        .is_none());
        assert!(load_workspace_instructions(None).is_none());
        assert!(load_workspace_instructions(Some(&context(Vec::new()))).is_none());
    }

    #[test]
    fn caps_total_size_and_flags_more_instructions_than_the_budget() {
        let cap = MAX_WORKSPACE_INSTRUCTIONS_BYTES;

        let exact = tempdir().unwrap();
        let later = tempdir().unwrap();
        fs::write(exact.path().join("AGENTS.md"), "a".repeat(cap)).unwrap();
        fs::write(later.path().join("AGENTS.md"), "later").unwrap();
        let loaded = load_workspace_instructions(Some(&context(vec![
            folder(exact.path(), "alpha", ProjectFolderRole::Primary),
            folder(later.path(), "beta", ProjectFolderRole::Auxiliary),
        ])))
        .unwrap();
        // A file that exactly consumes the budget is complete; anything after it is dropped and
        // reported as truncated.
        assert!(loaded.truncated);
        assert_eq!(loaded.sources.len(), 1);
        assert_eq!(loaded.sources[0].content.len(), cap);

        let oversized = tempdir().unwrap();
        fs::write(oversized.path().join("AGENTS.md"), "b".repeat(cap + 5)).unwrap();
        let loaded = load_workspace_instructions(Some(&context(vec![folder(
            oversized.path(),
            "gamma",
            ProjectFolderRole::Primary,
        )])))
        .unwrap();
        assert!(loaded.truncated);
        assert_eq!(loaded.sources[0].content.len(), cap);

        let exact_only = tempdir().unwrap();
        fs::write(exact_only.path().join("AGENTS.md"), "c".repeat(cap)).unwrap();
        let loaded = load_workspace_instructions(Some(&context(vec![folder(
            exact_only.path(),
            "delta",
            ProjectFolderRole::Primary,
        )])))
        .unwrap();
        assert!(!loaded.truncated);
        assert_eq!(loaded.sources[0].content.len(), cap);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_candidate_is_skipped_instead_of_read_outside_the_workspace() {
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("secret.md"), "outside secret").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.md"),
            workspace.path().join("AGENTS.md"),
        )
        .unwrap();
        assert!(
            load_workspace_instructions(Some(&context(vec![folder(
                workspace.path(),
                "app",
                ProjectFolderRole::Primary,
            )])))
            .is_none(),
            "a symlinked AGENTS.md must never contribute outside content"
        );

        // A symlinked override falls through to the real base file instead of redirecting, and
        // the outside content is still absent from the result.
        std::os::unix::fs::symlink(
            outside.path().join("secret.md"),
            workspace.path().join("AGENTS.override.md"),
        )
        .unwrap();
        fs::remove_file(workspace.path().join("AGENTS.md")).unwrap();
        fs::write(workspace.path().join("AGENTS.md"), "inside instructions").unwrap();
        let loaded = load_workspace_instructions(Some(&context(vec![folder(
            workspace.path(),
            "app",
            ProjectFolderRole::Primary,
        )])))
        .expect("the real base file still contributes");
        assert_eq!(loaded.sources[0].relative_path, "AGENTS.md");
        assert_eq!(loaded.sources[0].content, "inside instructions");
    }

    #[cfg(unix)]
    #[test]
    fn replaced_root_symlink_never_reads_outside_content() {
        let parent = tempdir().unwrap();
        let root = parent.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("AGENTS.md"), "outside secret").unwrap();
        let captured = folder(&root, "app", ProjectFolderRole::Primary);
        fs::remove_dir(&root).unwrap();
        std::os::unix::fs::symlink(outside.path(), &root).unwrap();
        assert!(
            load_workspace_instructions(Some(&context(vec![captured]))).is_none(),
            "a frozen root replaced by a symlink must not be followed"
        );
    }

    #[test]
    fn replaced_root_with_a_new_directory_is_rejected_by_directory_identity() {
        let parent = tempdir().unwrap();
        let root = parent.path().join("workspace");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("AGENTS.md"), "original instructions").unwrap();
        let captured = folder(&root, "app", ProjectFolderRole::Primary);
        fs::remove_dir_all(&root).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("AGENTS.md"), "replacement instructions").unwrap();
        assert!(
            load_workspace_instructions(Some(&context(vec![captured]))).is_none(),
            "directory identity must reject a same-path replacement"
        );
    }
}
