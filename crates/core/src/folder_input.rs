//! Host-owned references to folders selected in the composer.
//!
//! A folder reference is deliberately different from an attachment: selecting a folder does
//! not recursively upload its contents.  The Host keeps the authority (the canonical root and
//! its directory identity). The model sees the folder name and absolute path; selected folders
//! receive read access, while writes continue to follow the run's global permission.
//! Callers resolve a relative path on demand through [`AgentFolderAuthority`].

use crate::file_change::FileChangeDirectoryIdentity;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const AGENT_FOLDER_REFERENCE_SCHEMA_VERSION: u32 = 1;
pub const MAX_AGENT_FOLDER_REFERENCE_ID_CHARS: usize = 256;
pub const MAX_AGENT_FOLDER_REFERENCE_NAME_CHARS: usize = 512;
pub const MAX_AGENT_FOLDER_REFERENCE_PATH_CHARS: usize = 32 * 1024;
pub const MAX_AGENT_FOLDER_LIST_ENTRIES: usize = 2_048;
pub const MAX_AGENT_FOLDER_LIST_DEPTH: usize = 16;
pub const MAX_AGENT_FOLDER_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Durable identity for one folder selected by the user.
///
/// Model projections include the name and absolute path. The internal identity
/// is preserved when forking a task even when the folder is currently unavailable.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFolderReference {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    /// User-selected absolute path, visible to the model and used for ordinary tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_path: Option<String>,
    /// Frozen directory identity captured by Host when the reference is granted. This is kept
    /// with durable Host metadata but never included in model-facing projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_identity: Option<FileChangeDirectoryIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentFolderStatus>,
}

fn default_schema_version() -> u32 {
    AGENT_FOLDER_REFERENCE_SCHEMA_VERSION
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFolderStatus {
    Available,
    Unavailable,
}

impl AgentFolderReference {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Result<Self, String> {
        let reference = Self {
            schema_version: AGENT_FOLDER_REFERENCE_SCHEMA_VERSION,
            id: id.into(),
            name: name.into(),
            root_path: None,
            root_identity: None,
            status: None,
        };
        reference.validate()?;
        Ok(reference)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != AGENT_FOLDER_REFERENCE_SCHEMA_VERSION {
            return Err("unsupported Agent folder reference schema version".into());
        }
        validate_text(&self.id, MAX_AGENT_FOLDER_REFERENCE_ID_CHARS, "id")?;
        validate_text(&self.name, MAX_AGENT_FOLDER_REFERENCE_NAME_CHARS, "name")?;
        if self.id.contains('/') || self.id.contains('\\') || self.id == "." || self.id == ".." {
            return Err("Agent folder reference id must be an opaque single segment".into());
        }
        if let Some(root_path) = self.root_path.as_deref() {
            validate_text(
                root_path,
                MAX_AGENT_FOLDER_REFERENCE_PATH_CHARS,
                "root_path",
            )?;
            if !Path::new(root_path).is_absolute() {
                return Err("Agent folder reference root_path must be absolute".into());
            }
        }
        Ok(())
    }

    /// Human-readable location used by model-facing tools; never expose the internal id.
    pub fn model_path(&self) -> String {
        self.root_path.clone().unwrap_or_else(|| self.name.clone())
    }

    pub fn with_root_path(mut self, root_path: impl Into<String>) -> Self {
        self.root_path = Some(root_path.into());
        self
    }

    /// Captures the current directory identity when a fresh Host picker reference still has a
    /// usable root path. Unavailable roots are retained as opaque metadata so replay and fork do
    /// not fail merely because the source folder is temporarily offline.
    pub fn bind_root_identity_if_available(&mut self) {
        if self.root_identity.is_some() {
            return;
        }
        let Some(root) = self.root_path.as_deref() else {
            return;
        };
        match FileChangeDirectoryIdentity::read(Path::new(root)) {
            Ok(identity) => {
                self.root_identity = Some(identity);
                self.status = Some(AgentFolderStatus::Available);
            }
            Err(_) => {
                self.status = Some(AgentFolderStatus::Unavailable);
            }
        }
    }

    /// Returns the renderer/model-event form: display name plus absolute path, while preserving
    /// the stable internal id needed to reconcile queued guidance and persisted UI items. Host-only
    /// directory identity and availability metadata never leave the Host.
    pub fn model_projection(&self) -> Self {
        Self {
            schema_version: self.schema_version,
            id: self.id.clone(),
            name: self.name.clone(),
            root_path: self.root_path.clone(),
            root_identity: None,
            status: None,
        }
    }
}

pub fn bind_folder_references_for_storage(references: &mut [AgentFolderReference]) {
    for reference in references {
        reference.bind_root_identity_if_available();
    }
}

pub fn model_folder_references(references: &[AgentFolderReference]) -> Vec<AgentFolderReference> {
    references
        .iter()
        .map(AgentFolderReference::model_projection)
        .collect()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredFolderReference {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    id: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_identity: Option<FileChangeDirectoryIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<AgentFolderStatus>,
}

/// Serializes folder references for local storage. This is intentionally separate from the
/// model projection, which excludes Host-only directory identity metadata.
pub fn serialize_folder_references_for_storage(
    references: &[AgentFolderReference],
) -> Result<String, String> {
    references
        .iter()
        .map(|reference| {
            reference.validate()?;
            Ok(StoredFolderReference {
                schema_version: reference.schema_version,
                id: reference.id.clone(),
                name: reference.name.clone(),
                root_path: reference.root_path.clone(),
                root_identity: reference.root_identity.clone(),
                status: reference.status,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .and_then(|stored| serde_json::to_string(&stored).map_err(|error| error.to_string()))
}

/// Parses the local storage form while preserving the Host-only path for a later rebind.
pub fn deserialize_folder_references_from_storage(
    value: &str,
) -> Result<Vec<AgentFolderReference>, String> {
    let stored = serde_json::from_str::<Vec<StoredFolderReference>>(value)
        .map_err(|error| error.to_string())?;
    stored
        .into_iter()
        .map(|stored| {
            let reference = AgentFolderReference {
                schema_version: stored.schema_version,
                id: stored.id,
                name: stored.name,
                root_path: stored.root_path,
                root_identity: stored.root_identity,
                status: stored.status,
            };
            reference.validate()?;
            Ok(reference)
        })
        .collect()
}

/// Host-private authority associated with a folder reference.
///
/// Constructing an authority canonicalizes the selected root and freezes its device/inode (or
/// platform equivalent).  Every later resolution rechecks both, so a moved/replaced folder is
/// reported as unavailable instead of silently following a different directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFolderAuthority {
    reference: AgentFolderReference,
    canonical_root: PathBuf,
    directory_identity: FileChangeDirectoryIdentity,
}

impl AgentFolderAuthority {
    pub fn new(reference: AgentFolderReference, root: impl AsRef<Path>) -> Result<Self, String> {
        reference.validate()?;
        let root = root.as_ref();
        let metadata = fs::symlink_metadata(root)
            .map_err(|error| format!("folder is unavailable: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("folder reference must point to a real directory".into());
        }
        let canonical_root = root
            .canonicalize()
            .map_err(|error| format!("folder is unavailable: {error}"))?;
        let directory_identity = FileChangeDirectoryIdentity::read(&canonical_root)
            .map_err(|error| error.to_string())?;
        if let Some(expected) = reference.root_identity.as_ref() {
            expected.validate().map_err(|error| error.to_string())?;
            if expected != &directory_identity {
                return Err("folder reference root identity changed".to_string());
            }
        }
        let mut reference = reference;
        // Fresh picker references have no identity yet. Freeze the identity at the moment the
        // Host grants the authority so subsequent tool calls cannot silently rebind the path.
        if reference.root_identity.is_none() {
            reference.root_identity = Some(directory_identity.clone());
        }
        reference.status = Some(AgentFolderStatus::Available);
        Ok(Self {
            reference,
            canonical_root,
            directory_identity,
        })
    }

    pub fn reference(&self) -> &AgentFolderReference {
        &self.reference
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    /// Revalidates the selected root without resolving a child path. Consumers that perform a
    /// potentially long directory walk should call this again before returning results.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_root()
    }

    /// Revalidates the frozen root and resolves a relative path without following symlinks.
    pub fn resolve_relative(&self, relative: &str) -> Result<PathBuf, String> {
        validate_relative_path(relative)?;
        self.validate_root()?;
        let target = if relative.is_empty() {
            self.canonical_root.clone()
        } else {
            self.canonical_root.join(relative)
        };
        reject_symlink_components(&self.canonical_root, &target)?;
        let canonical = target
            .canonicalize()
            .map_err(|error| format!("folder path is unavailable: {error}"))?;
        if !canonical.starts_with(&self.canonical_root) {
            return Err("folder path escapes its selected root".into());
        }
        Ok(canonical)
    }

    /// Reads one regular file on demand.  Folder selections never cause a recursive upload.
    pub fn read_file(&self, relative: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        let path = self.resolve_relative(relative)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("folder path is not a regular file".into());
        }
        let max_bytes = max_bytes.min(MAX_AGENT_FOLDER_FILE_BYTES);
        if metadata.len() > max_bytes {
            return Err(format!("folder file exceeds the {max_bytes}-byte limit"));
        }
        let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
        let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
        file.read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > max_bytes {
            return Err(format!("folder file exceeds the {max_bytes}-byte limit"));
        }
        Ok(bytes)
    }

    /// Returns a bounded, deterministic directory listing for lazy model/tool inspection.
    pub fn list(
        &self,
        relative: &str,
        max_depth: usize,
        max_entries: usize,
    ) -> Result<Vec<AgentFolderEntry>, String> {
        let root = self.resolve_relative(relative)?;
        if !root.is_dir() {
            return Err("folder path is not a directory".into());
        }
        let max_depth = max_depth.min(MAX_AGENT_FOLDER_LIST_DEPTH);
        let max_entries = max_entries.min(MAX_AGENT_FOLDER_LIST_ENTRIES);
        let base = self.canonical_root.clone();
        let mut output = Vec::new();
        visit_directory(&base, &root, 0, max_depth, max_entries, &mut output)?;
        output.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(output)
    }

    fn validate_root(&self) -> Result<(), String> {
        let current = self
            .canonical_root
            .canonicalize()
            .map_err(|error| format!("folder is unavailable: {error}"))?;
        if current != self.canonical_root {
            return Err("folder reference root changed".into());
        }
        let identity =
            FileChangeDirectoryIdentity::read(&current).map_err(|error| error.to_string())?;
        if identity != self.directory_identity {
            return Err("folder reference root identity changed".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFolderEntryKind {
    Directory,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFolderEntry {
    pub relative_path: String,
    pub kind: AgentFolderEntryKind,
    pub size_bytes: Option<u64>,
}

fn visit_directory(
    base: &Path,
    directory: &Path,
    depth: usize,
    max_depth: usize,
    max_entries: usize,
    output: &mut Vec<AgentFolderEntry>,
) -> Result<(), String> {
    if output.len() >= max_entries {
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot list folder: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot list folder: {error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if output.len() >= max_entries {
            break;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let relative = path
            .strip_prefix(base)
            .map_err(|_| "folder entry escaped its selected root".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if metadata.is_dir() {
            output.push(AgentFolderEntry {
                relative_path: relative,
                kind: AgentFolderEntryKind::Directory,
                size_bytes: None,
            });
            if depth < max_depth {
                visit_directory(base, &path, depth + 1, max_depth, max_entries, output)?;
            }
        } else if metadata.is_file() {
            output.push(AgentFolderEntry {
                relative_path: relative,
                kind: AgentFolderEntryKind::File,
                size_bytes: Some(metadata.len()),
            });
        }
    }
    Ok(())
}

fn validate_text(value: &str, max_chars: usize, field: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.contains('\0')
        || value.contains('\n')
        || value.contains('\r')
        || value.chars().count() > max_chars
    {
        return Err(format!("folder reference {field} is invalid"));
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Ok(());
    }
    let path = Path::new(value);
    if path.is_absolute()
        || value.contains('\0')
        || value.contains('\n')
        || value.contains('\r')
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("folder relative path is invalid".into());
    }
    Ok(())
}

fn reject_symlink_components(root: &Path, target: &Path) -> Result<(), String> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| "folder path escapes its selected root".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err("folder relative path is invalid".into());
        };
        current.push(part);
        let metadata = fs::symlink_metadata(&current).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("folder path cannot traverse symbolic links".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn reference_exposes_path_but_keeps_identity_host_owned() {
        let reference = AgentFolderReference::new("folder-1", "Documents")
            .unwrap()
            .with_root_path("/private/Documents");
        let json = serde_json::to_value(&reference).unwrap();
        assert_eq!(json["id"], "folder-1");
        assert_eq!(json["rootPath"], "/private/Documents");
        let storage =
            serialize_folder_references_for_storage(std::slice::from_ref(&reference)).unwrap();
        assert!(storage.contains("/private/Documents"));
        assert_eq!(
            deserialize_folder_references_from_storage(&storage).unwrap(),
            vec![reference.clone()]
        );
        assert_eq!(reference.model_path(), "/private/Documents");
    }

    #[test]
    fn authority_rejects_a_persisted_reference_bound_to_a_different_directory() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        let mut reference = AgentFolderReference::new("folder-1", "Project")
            .unwrap()
            .with_root_path(first.path().to_string_lossy());
        reference.root_identity = Some(FileChangeDirectoryIdentity::read(first.path()).unwrap());
        assert!(AgentFolderAuthority::new(reference, second.path()).is_err());
    }

    #[test]
    fn reads_files_on_demand_and_lists_bounded_tree() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
        let authority = AgentFolderAuthority::new(
            AgentFolderReference::new("folder-1", "Project").unwrap(),
            root.path(),
        )
        .unwrap();
        assert_eq!(
            authority.read_file("src/main.rs", 64).unwrap(),
            b"fn main() {}"
        );
        let listing = authority.list("", 4, 10).unwrap();
        assert!(listing.iter().any(|entry| entry.relative_path == "src"));
        assert!(listing
            .iter()
            .any(|entry| entry.relative_path == "src/main.rs"));
        assert!(authority.resolve_relative("../outside").is_err());
    }

    #[test]
    fn replaced_root_becomes_unavailable() {
        let root = tempdir().unwrap();
        let reference = AgentFolderReference::new("folder-1", "Project").unwrap();
        let authority = AgentFolderAuthority::new(reference, root.path()).unwrap();
        let replacement = tempdir().unwrap();
        let old = root.path().to_path_buf();
        fs::remove_dir_all(&old).unwrap();
        fs::create_dir(&old).unwrap();
        fs::write(old.join("new.txt"), "new").unwrap();
        let result = authority.list("", 1, 10);
        assert!(result.is_err());
        drop(replacement);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folder_entries_are_never_resolved() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink(outside.path(), root.path().join("outside")).unwrap();
        let authority = AgentFolderAuthority::new(
            AgentFolderReference::new("folder-1", "Project").unwrap(),
            root.path(),
        )
        .unwrap();
        assert!(authority.read_file("outside/secret.txt", 64).is_err());
        assert!(authority
            .list("", 4, 10)
            .unwrap()
            .iter()
            .all(|entry| !entry.relative_path.starts_with("outside/")));
    }
}
