use super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const UNSUPPORTED_EXTENSIONS: &[&str] = &["pdf", "doc", "docx", "ppt", "pptx", "xls", "xlsx"];
const VCS_COMPONENTS: &[&str] = &[".git", ".hg", ".svn"];

#[derive(Debug, Clone)]
pub struct FileChangePathPolicy {
    workspace_root: Option<PathBuf>,
    allow_outside_workspace: bool,
}

impl FileChangePathPolicy {
    pub fn new(workspace_root: Option<&Path>, allow_outside_workspace: bool) -> Self {
        Self {
            workspace_root: workspace_root.map(Path::to_path_buf),
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

        let workspace_root = self
            .workspace_root
            .as_deref()
            .map(canonical_directory)
            .transpose()?;
        let expanded = crate::expand_system_path(input).map_err(|error| {
            FileChangeError::with_diagnostic(FileChangeErrorCode::InvalidArguments, error)
        })?;
        let supplied = expanded.unwrap_or_else(|| PathBuf::from(input));
        let unresolved = if supplied.is_absolute() {
            normalize_absolute(&supplied)?
        } else {
            let root = workspace_root
                .as_deref()
                .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::WorkspaceRequired))?;
            root.join(clean_relative(&supplied)?)
        };

        validate_existing_components_no_symlink(&unresolved)?;
        let parent = unresolved
            .parent()
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::ParentMissing))?;
        let canonical_parent = canonical_directory(parent)?;
        let parent_identity = DirectoryIdentity::read(&canonical_parent)?;
        if let Some(root) = workspace_root.as_deref() {
            if !self.allow_outside_workspace && !canonical_parent.starts_with(root) {
                return Err(FileChangeError::new(FileChangeErrorCode::PermissionDenied));
            }
        } else if !self.allow_outside_workspace {
            return Err(FileChangeError::new(FileChangeErrorCode::PermissionDenied));
        }
        let file_name = unresolved
            .file_name()
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
        let absolute_path = canonical_parent.join(file_name);
        reject_vcs_component(&absolute_path)?;
        reject_unsupported_extension(&absolute_path)?;
        validate_leaf(&absolute_path)?;

        let display_path = workspace_root
            .as_deref()
            .and_then(|root| absolute_path.strip_prefix(root).ok())
            .map(relative_display)
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| absolute_path.to_string_lossy().to_string());
        Ok(ResolvedFileChangeTarget {
            absolute_path,
            canonical_parent,
            display_path,
            parent_identity,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFileChangeTarget {
    absolute_path: PathBuf,
    canonical_parent: PathBuf,
    display_path: String,
    parent_identity: DirectoryIdentity,
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

fn clean_relative(path: &Path) -> FileChangeResultValue<PathBuf> {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FileChangeError::new(FileChangeErrorCode::PermissionDenied));
            }
        }
    }
    if cleaned.as_os_str().is_empty() {
        Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    } else {
        Ok(cleaned)
    }
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

fn relative_display(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}
