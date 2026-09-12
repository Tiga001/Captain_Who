use super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const UNSUPPORTED_EXTENSIONS: &[&str] = &["pdf", "doc", "docx", "ppt", "pptx", "xls", "xlsx"];
const VCS_COMPONENTS: &[&str] = &[".git", ".hg", ".svn"];

#[derive(Debug, Clone)]
pub struct FileChangePathPolicy {
    workspace_root: Option<PathBuf>,
    workspace: Option<crate::AgentWorkspaceContext>,
    allow_outside_workspace: bool,
}

impl FileChangePathPolicy {
    pub fn new(workspace_root: Option<&Path>, allow_outside_workspace: bool) -> Self {
        Self {
            workspace_root: workspace_root.map(Path::to_path_buf),
            workspace: None,
            allow_outside_workspace,
        }
    }

    /// Bind path resolution to the Host-frozen workspace, never current project settings.
    pub fn from_workspace(
        workspace: Option<&crate::AgentWorkspaceContext>,
        allow_outside_workspace: bool,
    ) -> Self {
        Self {
            workspace_root: workspace
                .and_then(|value| value.root_path.as_deref())
                .map(PathBuf::from),
            workspace: workspace.cloned(),
            allow_outside_workspace,
        }
    }

    pub fn resolve(&self, input: &str) -> FileChangeResultValue<ResolvedFileChangeTarget> {
        let input = input.trim();
        if input.is_empty() {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        reject_vcs_component(Path::new(input))?;
        reject_unsupported_extension(Path::new(input))?;

        let resolver = match self.workspace.as_ref() {
            Some(workspace) => crate::workspace::WorkspaceResolver::from_context(Some(workspace)),
            None => {
                let root = self
                    .workspace_root
                    .as_deref()
                    .map(canonical_directory)
                    .transpose()?;
                crate::workspace::WorkspaceResolver::from_primary(root.as_deref())
            }
        };
        if !Path::new(input).is_absolute()
            && !input.starts_with('@')
            && !input.starts_with('~')
            && resolver.primary_root().is_none()
        {
            return Err(FileChangeError::new(FileChangeErrorCode::WorkspaceRequired));
        }
        let supplied = resolver
            .resolve_input(input)
            .map_err(workspace_path_error)?;
        let unresolved = normalize_absolute(&supplied)?;

        validate_existing_components_no_symlink(&unresolved)?;
        let parent = unresolved
            .parent()
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::ParentMissing))?;
        let canonical_parent = canonical_directory(parent)?;
        let parent_identity = DirectoryIdentity::read(&canonical_parent)?;
        let workspace_root = resolver
            .containing_root(&canonical_parent)
            .map_err(workspace_path_error)?;
        if workspace_root.is_none() && !self.allow_outside_workspace {
            return Err(FileChangeError::new(FileChangeErrorCode::PermissionDenied));
        }
        let file_name = unresolved
            .file_name()
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
        let absolute_path = canonical_parent.join(file_name);
        reject_vcs_component(&absolute_path)?;
        reject_unsupported_extension(&absolute_path)?;
        validate_leaf(&absolute_path)?;

        let display_path = resolver.display_path(&absolute_path);
        let workspace_root_identity = workspace_root
            .map(|root| {
                let frozen = resolver
                    .folders()
                    .iter()
                    .find(|folder| {
                        folder.canonical_path.as_deref().map(Path::new) == Some(root.as_path())
                    })
                    .and_then(|folder| folder.directory_identity.clone());
                let identity = frozen
                    .map(Ok)
                    .unwrap_or_else(|| super::FileChangeDirectoryIdentity::read(&root))
                    .map_err(workspace_path_error)?;
                Ok::<_, FileChangeError>((root, identity))
            })
            .transpose()?;
        Ok(ResolvedFileChangeTarget {
            absolute_path,
            canonical_parent,
            display_path,
            parent_identity,
            workspace_root_identity,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFileChangeTarget {
    absolute_path: PathBuf,
    canonical_parent: PathBuf,
    display_path: String,
    parent_identity: DirectoryIdentity,
    workspace_root_identity: Option<(PathBuf, super::FileChangeDirectoryIdentity)>,
}

impl ResolvedFileChangeTarget {
    pub fn absolute_path(&self) -> &Path {
        &self.absolute_path
    }

    pub fn parent(&self) -> &Path {
        &self.canonical_parent
    }

    pub fn display_path(&self) -> &str {
        &self.display_path
    }

    pub(crate) fn revalidate(&self) -> FileChangeResultValue<()> {
        if let Some((root, identity)) = &self.workspace_root_identity {
            if super::FileChangeDirectoryIdentity::read(root).map_err(workspace_path_error)?
                != *identity
            {
                return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
            }
        }
        validate_existing_components_no_symlink(&self.absolute_path)?;
        let current_parent = canonical_directory(&self.canonical_parent)?;
        if current_parent != self.canonical_parent {
            return Err(FileChangeError::new(FileChangeErrorCode::SymlinkForbidden));
        }
        if DirectoryIdentity::read(&current_parent)? != self.parent_identity {
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
        }
        validate_leaf(&self.absolute_path)
    }
}

fn workspace_path_error(error: impl ToString) -> FileChangeError {
    FileChangeError::with_diagnostic(FileChangeErrorCode::PermissionDenied, error.to_string())
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl DirectoryIdentity {
    fn read(path: &Path) -> FileChangeResultValue<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| path_io_error(error, FileChangeErrorCode::ParentMissing))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(FileChangeError::new(FileChangeErrorCode::SymlinkForbidden));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[cfg(not(unix))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryIdentity {
    canonical_path: PathBuf,
}

#[cfg(not(unix))]
impl DirectoryIdentity {
    fn read(path: &Path) -> FileChangeResultValue<Self> {
        Ok(Self {
            canonical_path: canonical_directory(path)?,
        })
    }
}

fn canonical_directory(path: &Path) -> FileChangeResultValue<PathBuf> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| path_io_error(error, FileChangeErrorCode::ParentMissing))?;
    if metadata.file_type().is_symlink() {
        return Err(FileChangeError::new(FileChangeErrorCode::SymlinkForbidden));
    }
    if !metadata.is_dir() {
        return Err(FileChangeError::new(FileChangeErrorCode::ParentMissing));
    }
    path.canonicalize()
        .map_err(|error| path_io_error(error, FileChangeErrorCode::ParentMissing))
}

fn normalize_absolute(path: &Path) -> FileChangeResultValue<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(FileChangeError::new(FileChangeErrorCode::PermissionDenied));
            }
        }
    }
    if normalized.is_absolute() {
        Ok(normalized)
    } else {
        Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    }
}

fn validate_existing_components_no_symlink(path: &Path) -> FileChangeResultValue<()> {
    let mut current = PathBuf::new();
    let components = path.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(path_io_error(error, FileChangeErrorCode::ParentMissing));
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(FileChangeError::new(FileChangeErrorCode::SymlinkForbidden));
        }
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err(FileChangeError::new(FileChangeErrorCode::ParentMissing));
        }
    }
    Ok(())
}

fn validate_leaf(path: &Path) -> FileChangeResultValue<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(path_io_error(error, FileChangeErrorCode::FileMissing)),
    };
    if metadata.file_type().is_symlink() {
        return Err(FileChangeError::new(FileChangeErrorCode::SymlinkForbidden));
    }
    if !metadata.is_file() {
        return Err(FileChangeError::new(FileChangeErrorCode::NotRegularFile));
    }
    reject_hard_link(&metadata)
}

fn path_io_error(error: io::Error, missing: FileChangeErrorCode) -> FileChangeError {
    let code = match error.kind() {
        io::ErrorKind::NotFound => missing,
        io::ErrorKind::PermissionDenied => FileChangeErrorCode::PermissionDenied,
        _ => FileChangeErrorCode::Failed,
    };
    FileChangeError::with_diagnostic(code, error.to_string())
}

#[cfg(unix)]
fn reject_hard_link(metadata: &fs::Metadata) -> FileChangeResultValue<()> {
    use std::os::unix::fs::MetadataExt;
    if metadata.nlink() == 1 {
        Ok(())
    } else {
        Err(FileChangeError::new(FileChangeErrorCode::HardLinkForbidden))
    }
}

#[cfg(not(unix))]
fn reject_hard_link(_metadata: &fs::Metadata) -> FileChangeResultValue<()> {
    Ok(())
}

fn reject_vcs_component(path: &Path) -> FileChangeResultValue<()> {
    if path.components().any(|component| {
        let Component::Normal(part) = component else {
            return false;
        };
        VCS_COMPONENTS.iter().any(|blocked| part == *blocked)
    }) {
        Err(FileChangeError::new(FileChangeErrorCode::PermissionDenied))
    } else {
        Ok(())
    }
}

fn reject_unsupported_extension(path: &Path) -> FileChangeResultValue<()> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if UNSUPPORTED_EXTENSIONS
        .iter()
        .any(|unsupported| extension == *unsupported)
    {
        Err(FileChangeError::new(
            FileChangeErrorCode::UnsupportedFileType,
        ))
    } else {
        Ok(())
    }
}
