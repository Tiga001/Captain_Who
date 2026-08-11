use super::*;
use crate::command::ManagedCommandWorkspaceLease;
use sha2::{Digest, Sha256};
use std::time::Duration;

const MAX_SCOPE_ID_BYTES: usize = 512;
const ORPHAN_MINIMUM_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const ORPHAN_SCAN_LIMIT: usize = 256;
const ORPHAN_REMOVAL_LIMIT: usize = 32;

impl StorageService {
    /// Acquires the host-private workspace shared by trusted managed commands in one Agent Run.
    ///
    /// Neither identity nor the derived physical path is accepted from the model. The hash keeps
    /// conversation/run identifiers out of filesystem names while remaining stable across an
    /// approval pause or application restart.
    pub fn acquire_managed_command_workspace(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<ManagedCommandWorkspaceLease, String> {
        validate_scope_identity("conversation", conversation_id)?;
        validate_scope_identity("run", run_id)?;
        self.managed_command_workspaces
            .acquire(&scope_digest(conversation_id, run_id))
    }

    pub fn cleanup_managed_command_workspace(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<(), String> {
        validate_scope_identity("conversation", conversation_id)?;
        validate_scope_identity("run", run_id)?;
        self.managed_command_workspaces
            .request_cleanup(&scope_digest(conversation_id, run_id))
    }

    pub(super) fn prune_stale_managed_command_workspaces(&self) -> Result<usize, String> {
        self.managed_command_workspaces.prune_stale(
            ORPHAN_MINIMUM_AGE,
            ORPHAN_SCAN_LIMIT,
            ORPHAN_REMOVAL_LIMIT,
        )
    }
}

fn validate_scope_identity(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_SCOPE_ID_BYTES || value.chars().any(char::is_control) {
        return Err(format!("受管命令 {label} 身份无效。"));
    }
    Ok(())
}

fn scope_digest(conversation_id: &str, run_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mycopilot-next/managed-command-workspace/v1\0");
    hasher.update((conversation_id.len() as u64).to_be_bytes());
    hasher.update(conversation_id.as_bytes());
    hasher.update((run_id.len() as u64).to_be_bytes());
    hasher.update(run_id.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_run_reopens_the_same_private_workspace_without_exposing_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
        let first = storage
            .acquire_managed_command_workspace("conversation-sensitive", "run-sensitive")
            .unwrap();
        let first_root = first.execution_root().to_path_buf();
        drop(first);
        let second = storage
            .acquire_managed_command_workspace("conversation-sensitive", "run-sensitive")
            .unwrap();
        assert_eq!(first_root, second.execution_root());
        assert!(!first_root
            .to_string_lossy()
            .contains("conversation-sensitive"));
        assert!(!first_root.to_string_lossy().contains("run-sensitive"));
        assert!(second.outputs_root().is_dir());
    }

    #[test]
    fn workspace_survives_storage_restart_until_terminal_cleanup_is_requested() {
        let fixture = tempfile::tempdir().unwrap();
        let database = fixture.path().join("storage.sqlite");
        let execution_root = {
            let storage = StorageService::open(&database).unwrap();
            let lease = storage
                .acquire_managed_command_workspace("conversation", "run")
                .unwrap();
            fs::write(
                lease.execution_root().join("intermediate.txt"),
                b"preserved",
            )
            .unwrap();
            lease.execution_root().to_path_buf()
        };
        assert!(execution_root.join("intermediate.txt").is_file());

        let storage = StorageService::open(&database).unwrap();
        let lease = storage
            .acquire_managed_command_workspace("conversation", "run")
            .unwrap();
        assert_eq!(
            fs::read(lease.execution_root().join("intermediate.txt")).unwrap(),
            b"preserved"
        );
        storage
            .cleanup_managed_command_workspace("conversation", "run")
            .unwrap();
        assert!(lease.execution_root().is_dir());
        drop(lease);
        assert!(!execution_root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_scope_directory() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
        let first = storage
            .acquire_managed_command_workspace("seed", "seed")
            .unwrap();
        let version_root = first.execution_root().parent().unwrap().to_path_buf();
        drop(first);
        let escaped = fixture.path().join("escaped");
        fs::create_dir(&escaped).unwrap();
        let scope = version_root.join(scope_digest("conversation", "run"));
        symlink(&escaped, &scope).unwrap();

        assert!(storage
            .acquire_managed_command_workspace("conversation", "run")
            .unwrap_err()
            .contains("非符号链接"));
    }
}
