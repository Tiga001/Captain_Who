use crate::{
    AgentFileChangeOperation, AgentFileChangeProposal, AgentRunContext, AgentWritePermission,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::fs::{File, OpenOptions};
#[cfg(windows)]
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, SECURITY_IDENTIFICATION,
    },
};

pub const FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION: u32 = 1;
pub const APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION: &str = "apply-patch-file-change-v1";

pub fn file_change_workspace_identity(
    project_id: Option<&str>,
    workspace_project_id: Option<&str>,
    canonical_root: &Path,
) -> Result<String, serde_json::Error> {
    super::proposal_digest(&serde_json::json!({
        "projectId": project_id,
        "workspaceProjectId": workspace_project_id,
        "canonicalRoot": canonical_root.to_string_lossy(),
    }))
}

fn deserialize_run_grant_schema_version<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version == FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(
            "unsupported FileChange run grant schema version",
        ))
    }
}

/// Private checkpoint reference to one Host-owned active Run grant.
///
/// This reference is not authority by itself. The Host reloads the exact durable record and
/// revalidates owner, scope, write ceiling, contract revision, and lifecycle immediately before
/// the FileChange dispatch claim.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChangeRunGrantRef {
    #[serde(deserialize_with = "deserialize_run_grant_schema_version")]
    pub schema_version: u32,
    pub grant_id: String,
    pub revision: u64,
    pub apply_patch_contract_revision: String,
}

impl FileChangeRunGrantRef {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION
            || !valid_grant_owned_id(&self.grant_id, 256)
            || self.revision == 0
            || self.apply_patch_contract_revision != APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION
        {
            return Err("invalid FileChange run grant reference");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeRunGrantScopeKind {
    Workspace,
    ExternalParent,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeRunGrantStatus {
    Pending,
    Active,
    Inactive,
    Revoked,
}

/// Scope derived from the exact frozen FileChange and its Host-persisted Run authority.
///
/// This value is deliberately not stored as an independent source of truth. Writers use the
/// derivation to materialize a grant row, while readers derive it again from the granting Pending
/// Action before treating an active row as authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedFileChangeRunGrantScope {
    pub scope_kind: FileChangeRunGrantScopeKind,
    pub workspace_identity: Option<String>,
    pub canonical_scope_path: String,
    pub scope_directory_identity: FileChangeDirectoryIdentity,
    pub base_write_permission: AgentWritePermission,
    /// Frozen workspace members; each target root is checked when deriving its scope, while
    /// the originally granting root is checked when reusing an active grant across roots.
    workspace_roots: Vec<(String, FileChangeDirectoryIdentity)>,
}

impl DerivedFileChangeRunGrantScope {
    pub fn matches_record(&self, record: &FileChangeRunGrantRecord) -> bool {
        self.scope_kind == record.scope_kind
            && self.workspace_identity == record.workspace_identity
            && self.canonical_scope_path == record.canonical_scope_path
            && self.scope_directory_identity == record.scope_directory_identity
            && self.base_write_permission == record.base_write_permission
    }

    pub fn is_authorized_by(&self, record: &FileChangeRunGrantRecord) -> bool {
        if self.scope_kind != record.scope_kind
            || self.workspace_identity != record.workspace_identity
            || self.base_write_permission != record.base_write_permission
        {
            return false;
        }
        match self.scope_kind {
            FileChangeRunGrantScopeKind::Workspace => {
                self.workspace_roots.iter().any(|(root, identity)| {
                    root == &record.canonical_scope_path
                        && identity == &record.scope_directory_identity
                        && FileChangeDirectoryIdentity::read(Path::new(root)).as_ref()
                            == Ok(identity)
                })
            }
            FileChangeRunGrantScopeKind::ExternalParent => {
                self.canonical_scope_path == record.canonical_scope_path
                    && self.scope_directory_identity == record.scope_directory_identity
            }
        }
    }
}

/// Derives the only current Run-grant scope from one exact create/update proposal.
///
/// The target is never inferred from the user-facing path. It comes from the frozen execution
/// binding, and every owner dimension must agree with the persisted Run context. Missing or
/// unavailable filesystem identity fails closed.
pub fn derive_file_change_run_grant_scope(
    proposal: &AgentFileChangeProposal,
    run_context: &AgentRunContext,
) -> Result<DerivedFileChangeRunGrantScope, &'static str> {
    if !matches!(
        proposal.operation,
        AgentFileChangeOperation::Create | AgentFileChangeOperation::Update
    ) {
        return Err("FileChange Run grants require create or update");
    }
    proposal
        .execution
        .validate()
        .map_err(|_| "FileChange Run grant execution binding is invalid")?;
    if proposal.execution.source_tool_name != "apply_patch"
        || proposal.execution.source_call_id != proposal.id
        || run_context.conversation_id.as_deref()
            != Some(proposal.execution.conversation_id.as_str())
        || run_context.project_id.as_deref() != proposal.execution.project_id.as_deref()
    {
        return Err("FileChange Run grant owner is invalid");
    }

    let base_write_permission = run_context.permissions.write;
    if base_write_permission == AgentWritePermission::Denied {
        return Err("FileChange Run grant write permission is denied");
    }
    let canonical_target = PathBuf::from(&proposal.execution.canonical_target);
    if !canonical_target.is_absolute() {
        return Err("FileChange Run grant target is invalid");
    }
    let canonical_parent = canonical_target
        .parent()
        .ok_or("FileChange Run grant target has no parent")?
        .canonicalize()
        .map_err(|_| "FileChange Run grant target parent is unavailable")?;
    let resolver =
        crate::workspace::WorkspaceResolver::from_context(run_context.workspace.as_ref());
    let workspace = resolver
        .containing_root(&canonical_target)
        .map_err(|_| "FileChange Run grant workspace is unavailable")?;
    let mut workspace_roots = resolver
        .folders()
        .iter()
        .filter_map(|folder| {
            Some((
                folder.canonical_path.clone()?,
                folder.directory_identity.clone()?,
            ))
        })
        .collect::<Vec<_>>();
    let (scope_kind, workspace_identity, canonical_scope_path) = match workspace {
        Some(workspace_root) if canonical_target.starts_with(&workspace_root) => {
            let identity = if resolver.folders().is_empty() {
                file_change_workspace_identity(
                    run_context.project_id.as_deref(),
                    run_context
                        .workspace
                        .as_ref()
                        .and_then(|workspace| workspace.project_id.as_deref()),
                    &workspace_root,
                )
            } else {
                super::proposal_digest(&serde_json::json!({
                    "projectId": run_context.project_id,
                    "workspace": run_context.workspace,
                }))
            }
            .map_err(|_| "FileChange Run grant workspace identity is invalid")?;
            if resolver.folders().is_empty() {
                workspace_roots.push((
                    workspace_root.to_string_lossy().into_owned(),
                    FileChangeDirectoryIdentity::read(&workspace_root)?,
                ));
            }
            (
                FileChangeRunGrantScopeKind::Workspace,
                Some(identity),
                workspace_root.to_string_lossy().into_owned(),
            )
        }
        _ if base_write_permission == AgentWritePermission::All => (
            FileChangeRunGrantScopeKind::ExternalParent,
            None,
            canonical_parent.to_string_lossy().into_owned(),
        ),
        _ => return Err("FileChange Run grant target is outside its workspace"),
    };
    let scope_directory_identity =
        FileChangeDirectoryIdentity::read(Path::new(&canonical_scope_path))?;
    if scope_kind == FileChangeRunGrantScopeKind::Workspace
        && !workspace_roots.iter().any(|(root, identity)| {
            root == &canonical_scope_path && identity == &scope_directory_identity
        })
    {
        return Err("FileChange Run grant workspace identity changed");
    }
    Ok(DerivedFileChangeRunGrantScope {
        scope_kind,
        workspace_identity,
        canonical_scope_path,
        scope_directory_identity,
        base_write_permission,
        workspace_roots,
    })
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileChangeDirectoryIdentity {
    Unix {
        schema_version: u32,
        device: u64,
        inode: u64,
    },
    Windows {
        schema_version: u32,
        volume_serial_number: u64,
        /// Lowercase hexadecimal FILE_ID_128 (exactly 16 bytes / 32 digits).
        file_id: String,
    },
}

impl FileChangeDirectoryIdentity {
    /// Builds the stable identity of an already-open Unix directory from that descriptor's
    /// metadata. Observation issuance uses this path so an ancestor cannot be swapped between a
    /// pathname lookup and identity capture.
    #[cfg(unix)]
    pub(crate) fn from_bound_metadata(metadata: &std::fs::Metadata) -> Result<Self, &'static str> {
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_dir() {
            return Err("FileChange directory identity requires a directory");
        }
        Ok(Self::Unix {
            schema_version: FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    pub fn read(path: &Path) -> Result<Self, &'static str> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| "FileChange grant directory is unavailable")?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("FileChange grant directory is not a real directory");
        }
        #[cfg(unix)]
        {
            Self::from_bound_metadata(&metadata)
        }
        #[cfg(windows)]
        {
            let _ = metadata;
            let file = open_windows_directory_identity(path)
                .map_err(|_| "FileChange grant directory identity is unavailable")?;
            let (volume_serial_number, file_id) = windows_directory_identity(&file)
                .map_err(|_| "FileChange grant directory identity is unavailable")?;
            Ok(Self::Windows {
                schema_version: FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
                volume_serial_number,
                file_id: hex_file_id(file_id),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = metadata;
            Err("FileChange Run grants require a supported stable directory identity")
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Unix { schema_version, .. }
                if *schema_version == FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION =>
            {
                Ok(())
            }
            Self::Windows {
                schema_version,
                file_id,
                ..
            } if *schema_version == FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION
                && file_id.len() == 32
                && file_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
            {
                Ok(())
            }
            _ => Err("invalid FileChange grant directory identity"),
        }
    }
}

#[cfg(windows)]
fn open_windows_directory_identity(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .security_qos_flags(SECURITY_IDENTIFICATION);
    options.open(path)
}

#[cfg(windows)]
fn windows_directory_identity(file: &File) -> std::io::Result<(u64, [u8; 16])> {
    let mut information = FILE_ID_INFO::default();
    // SAFETY: `file` owns a live HANDLE and `information` is an exactly sized writable output
    // buffer for FILE_ID_INFO.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as HANDLE,
            FileIdInfo,
            (&raw mut information).cast(),
            u32::try_from(std::mem::size_of::<FILE_ID_INFO>()).unwrap_or(u32::MAX),
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((
        information.VolumeSerialNumber,
        information.FileId.Identifier,
    ))
}

#[cfg(windows)]
fn hex_file_id(file_id: [u8; 16]) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(32);
    for byte in file_id {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

/// Host-only durable grant record. It is never projected to the model or Renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChangeRunGrantRecord {
    pub schema_version: u32,
    pub grant_id: String,
    pub revision: u64,
    pub status: FileChangeRunGrantStatus,
    pub run_id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub scope_kind: FileChangeRunGrantScopeKind,
    pub workspace_identity: Option<String>,
    pub canonical_scope_path: String,
    pub scope_directory_identity: FileChangeDirectoryIdentity,
    /// Canonical v2 Pending Action storage id (`run_id` + Provider call id), never a bare
    /// Provider Tool Call id which may repeat in another Run.
    pub granting_pending_action_id: String,
    pub base_write_permission: AgentWritePermission,
    pub granting_permission_revision: String,
    pub granting_tool_set_revision: String,
    pub granting_provider_wire_revision: String,
    pub apply_patch_contract_revision: String,
    /// Digest of the exact typed terminal FileChange result that activated this grant. The raw
    /// result remains in the private action audit; an active grant is authority only while both
    /// records still agree.
    pub activation_result_digest: Option<String>,
    pub created_at: i64,
    pub activated_at: Option<i64>,
    pub inactive_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl FileChangeRunGrantRecord {
    pub fn reference(&self) -> Result<FileChangeRunGrantRef, &'static str> {
        self.validate()?;
        if self.status != FileChangeRunGrantStatus::Active {
            return Err("FileChange run grant is not active");
        }
        Ok(FileChangeRunGrantRef {
            schema_version: self.schema_version,
            grant_id: self.grant_id.clone(),
            revision: self.revision,
            apply_patch_contract_revision: self.apply_patch_contract_revision.clone(),
        })
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION
            || !valid_grant_owned_id(&self.grant_id, 256)
            || !valid_grant_owned_id(&self.run_id, 256)
            || self.conversation_id.trim().is_empty()
            || self.canonical_scope_path.trim().is_empty()
            || self.canonical_scope_path.trim() != self.canonical_scope_path
            || self.canonical_scope_path.chars().any(char::is_control)
            || !crate::is_canonical_pending_action_id_for_run(
                &self.granting_pending_action_id,
                &self.run_id,
            )
            || self.apply_patch_contract_revision != APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION
            || !valid_grant_owned_id(&self.granting_permission_revision, 256)
            || !valid_grant_owned_id(&self.granting_tool_set_revision, 256)
            || !valid_grant_owned_id(&self.granting_provider_wire_revision, 256)
            || self.base_write_permission == AgentWritePermission::Denied
        {
            return Err("invalid FileChange run grant");
        }
        self.scope_directory_identity.validate()?;
        match (self.scope_kind, self.workspace_identity.as_deref()) {
            (FileChangeRunGrantScopeKind::Workspace, Some(identity)) if !identity.is_empty() => {}
            (FileChangeRunGrantScopeKind::ExternalParent, None) => {}
            _ => return Err("invalid FileChange run grant scope"),
        }
        match self.status {
            FileChangeRunGrantStatus::Pending
                if self.revision == 0
                    && self.activation_result_digest.is_none()
                    && self.activated_at.is_none()
                    && self.inactive_at.is_none()
                    && self.revoked_at.is_none() => {}
            FileChangeRunGrantStatus::Active
                if self.revision > 0
                    && self
                        .activation_result_digest
                        .as_deref()
                        .is_some_and(super::valid_digest)
                    && self.activated_at.is_some()
                    && self.inactive_at.is_none()
                    && self.revoked_at.is_none() => {}
            FileChangeRunGrantStatus::Inactive
                if self.revision > 0
                    && self.activation_result_digest.is_none()
                    && self.activated_at.is_none()
                    && self.inactive_at.is_some()
                    && self.revoked_at.is_none() => {}
            FileChangeRunGrantStatus::Revoked if self.revision > 0 && self.revoked_at.is_some() => {
                if self
                    .activation_result_digest
                    .as_deref()
                    .is_some_and(|digest| !super::valid_digest(digest))
                {
                    return Err("invalid FileChange run grant activation result digest");
                }
            }
            _ => return Err("invalid FileChange run grant lifecycle"),
        }
        Ok(())
    }
}

fn valid_grant_owned_id(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_record() -> FileChangeRunGrantRecord {
        FileChangeRunGrantRecord {
            schema_version: FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
            grant_id: "grant-1".to_string(),
            revision: 0,
            status: FileChangeRunGrantStatus::Pending,
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            project_id: None,
            scope_kind: FileChangeRunGrantScopeKind::ExternalParent,
            workspace_identity: None,
            canonical_scope_path: "/tmp".to_string(),
            scope_directory_identity: FileChangeDirectoryIdentity::Unix {
                schema_version: FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
                device: 1,
                inode: 2,
            },
            granting_pending_action_id: crate::canonical_pending_action_id("run-1", "call-1"),
            base_write_permission: AgentWritePermission::All,
            granting_permission_revision: "permission-v1".to_string(),
            granting_tool_set_revision: "tool-set-v1".to_string(),
            granting_provider_wire_revision: "provider-wire-v1".to_string(),
            apply_patch_contract_revision: APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION.to_string(),
            activation_result_digest: None,
            created_at: 1,
            activated_at: None,
            inactive_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn references_and_records_reject_noncanonical_owned_ids() {
        let reference = FileChangeRunGrantRef {
            schema_version: FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
            grant_id: " grant-1".to_string(),
            revision: 1,
            apply_patch_contract_revision: APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION.to_string(),
        };
        assert!(reference.validate().is_err());

        for value in [" run-1", "run-1\n"] {
            let mut record = pending_record();
            record.run_id = value.to_string();
            record.granting_pending_action_id =
                crate::canonical_pending_action_id(&record.run_id, "call-1");
            assert!(record.validate().is_err());
        }
    }
}
