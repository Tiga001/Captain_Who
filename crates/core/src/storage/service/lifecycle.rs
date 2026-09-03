use super::*;

pub struct StorageService {
    pub(super) state: StorageState,
    pub(super) attachment_root: PathBuf,
    pub(super) image_artifact_root: PathBuf,
    pub(super) managed_artifact_root: PathBuf,
    pub(super) managed_command_workspaces: ManagedCommandWorkspaceRegistry,
    pub(super) model_credentials:
        Arc<dyn crate::image_generation::credential_store::CredentialStore>,
    pub(super) model_credential_lock: Mutex<()>,
}

impl StorageService {
    pub fn subscribe_storage_events(
        &self,
        stream: crate::storage::StorageEventStream,
    ) -> tokio::sync::watch::Receiver<u64> {
        self.state.event_notifications().subscribe(stream)
    }

    pub fn open(database_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if !cfg!(debug_assertions) {
            return Err("production storage requires an explicit model credential backend".into());
        }
        Self::open_with_model_credentials(
            database_path,
            Arc::new(crate::image_generation::credential_store::InMemoryCredentialStore::default()),
        )
    }

    /// Opens storage with the Host-owned credential backend used for language-model and search
    /// provider secrets. Production callers must inject a native store; tests and isolated tools
    /// may use an in-memory store explicitly.
    pub fn open_with_model_credentials(
        database_path: &Path,
        model_credentials: Arc<dyn crate::image_generation::credential_store::CredentialStore>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::open_internal(database_path, true, model_credentials)
    }

    /// Opens the strict storage boundary without touching sibling filesystem stores.
    ///
    /// This is intentionally limited to the explicit development reset tool, which operates on
    /// a staging database while holding the same process-wide database lock as Core Server. It
    /// must not run ordinary startup pruning before the new database is atomically published.
    pub fn open_for_development_reset(
        database_path: &Path,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::open_for_development_reset_with_model_credentials(
            database_path,
            Arc::new(crate::image_generation::credential_store::InMemoryCredentialStore::default()),
        )
    }

    pub fn open_for_development_reset_with_model_credentials(
        database_path: &Path,
        model_credentials: Arc<dyn crate::image_generation::credential_store::CredentialStore>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::open_internal(database_path, false, model_credentials)
    }

    fn open_internal(
        database_path: &Path,
        run_startup_maintenance: bool,
        model_credentials: Arc<dyn crate::image_generation::credential_store::CredentialStore>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
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
            model_credentials,
            model_credential_lock: Mutex::new(()),
        };

        if run_startup_maintenance {
            if let Err(error) = service.prune_stale_managed_command_workspaces() {
                eprintln!("failed to prune stale managed command workspaces: {error}");
            }

            match service.state.connection() {
                Ok(mut connection) => {
                    if let Err(error) = file_change_repository::expire_and_prune_file_changes(
                        &mut connection,
                        now_ms(),
                    ) {
                        eprintln!("failed to prune expired file changes: {error}");
                    }
                }
                Err(error) => eprintln!("failed to open storage for startup maintenance: {error}"),
            }
            if let Err(error) = service.cleanup_orphan_attachment_files() {
                eprintln!("failed to cleanup orphan attachment files: {error}");
            }
            match service.state.connection() {
                Ok(mut connection) => {
                    if let Err(error) =
                        context_compaction_receipt_repository::mark_in_progress_receipts_interrupted(
                            &mut connection,
                            now_ms(),
                        )
                    {
                        eprintln!("failed to mark interrupted context compactions: {error}");
                    }
                }
                Err(error) => {
                    eprintln!("failed to reopen storage for startup maintenance: {error}")
                }
            }
        }

        Ok(service)
    }
}
