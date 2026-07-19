use super::*;

pub struct StorageService {
    pub(super) state: StorageState,
    pub(super) attachment_root: PathBuf,
}

impl StorageService {
    pub fn open(database_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let attachment_root = database_path
            .parent()
            .map(|parent| parent.join("attachments"))
            .unwrap_or_else(|| PathBuf::from("attachments"));

        let service = Self {
            state: StorageState::open(database_path)?,
            attachment_root,
        };

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
