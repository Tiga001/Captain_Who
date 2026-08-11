use super::*;

pub struct StorageService {
    pub(super) state: StorageState,
    pub(super) attachment_root: PathBuf,
    pub(super) image_artifact_root: PathBuf,
    pub(super) managed_artifact_root: PathBuf,
    pub(super) managed_command_workspaces: ManagedCommandWorkspaceRegistry,
}

impl StorageService {
    pub fn open(database_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let attachment_root = database_path
            .parent()
            .map(|parent| parent.join("attachments"))
            .unwrap_or_else(|| PathBuf::from("attachments"));
        let image_artifact_root = database_path
            .parent()
            .map(|parent| parent.join("image-generation-artifacts"))
            .unwrap_or_else(|| PathBuf::from("image-generation-artifacts"));
        // Generic managed-command Artifacts share the existing immutable content-addressed
        // object directory. Their separate journal adds consumer authority; it is not a second
        // PDF- or command-specific file library.
        let managed_artifact_root = image_artifact_root.clone();
        let managed_command_workspace_root = database_path
            .parent()
            .map(|parent| parent.join("managed-command-runs"))
            .unwrap_or_else(|| PathBuf::from("managed-command-runs"))
            .join("v1");

        let service = Self {
            state: StorageState::open(database_path)?,
            attachment_root,
            image_artifact_root,
            managed_artifact_root,
            managed_command_workspaces: ManagedCommandWorkspaceRegistry::new(
                managed_command_workspace_root,
            ),
        };

        if let Err(error) = service.prune_stale_managed_command_workspaces() {
            eprintln!("failed to prune stale managed command workspaces: {error}");
        }

        match service.state.connection() {
            Ok(mut connection) => {
                if let Err(error) =
                    file_draft_repository::expire_and_prune_drafts(&mut connection, now_ms())
                {
                    eprintln!("failed to prune expired file drafts: {error}");
                }
                if let Err(error) = service.cleanup_orphan_attachment_files(&connection) {
                    eprintln!("failed to cleanup orphan attachment files: {error}");
                }
                if let Err(error) =
                    context_compaction_receipt_repository::mark_in_progress_receipts_interrupted(
                        &mut connection,
                        now_ms(),
                    )
                {
                    eprintln!("failed to mark interrupted context compactions: {error}");
                }
            }
            Err(error) => eprintln!("failed to open storage for startup maintenance: {error}"),
        }

        Ok(service)
    }
}
