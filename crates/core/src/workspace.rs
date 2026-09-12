//! Frozen workspace membership and model-facing filesystem addressing.
//! Resolution chooses a root, but leaves leaf/ancestor/scope checks to each operation.
use crate::file_change::FileChangeDirectoryIdentity;
use crate::storage::models::{ProjectFolderRole, ProjectRecord};
use crate::AgentWorkspaceContext;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceFolder {
    pub id: String,
    pub alias: String,
    pub role: ProjectFolderRole,
    pub path: String,
    pub canonical_path: Option<String>,
    pub directory_identity: Option<FileChangeDirectoryIdentity>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceResolver {
    context: Option<AgentWorkspaceContext>,
}

impl WorkspaceResolver {
    pub fn from_context(workspace: Option<&AgentWorkspaceContext>) -> Self {
        Self {
            context: workspace.cloned(),
        }
    }

    /// Explicit single-directory Host contexts (private workspaces and low-level callers).
    pub fn from_primary(root: Option<&Path>) -> Self {
        Self {
            context: root.map(|path| AgentWorkspaceContext {
                project_id: None,
                display_name: None,
                root_path: Some(path.to_string_lossy().into_owned()),
                folders: Vec::new(),
            }),
        }
    }

    pub fn context(&self) -> Option<&AgentWorkspaceContext> {
        self.context.as_ref()
    }

    pub fn primary_root(&self) -> Option<&Path> {
        self.context
            .as_ref()
            .and_then(|v| v.root_path.as_deref())
            .map(Path::new)
    }

    pub fn folders(&self) -> &[WorkspaceFolder] {
        self.context
            .as_ref()
            .map(|v| v.folders.as_slice())
            .unwrap_or(&[])
    }

    pub fn validate_shape(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        let mut aliases = BTreeSet::new();
        let folders = self.folders();
        if folders.len() > 32 {
            return Err("工作区文件夹数量无效。".into());
        }
        if folders.is_empty() {
            return Ok(());
        }
        let primaries: Vec<_> = folders
            .iter()
            .filter(|f| f.role == ProjectFolderRole::Primary)
            .collect();
        if primaries.len() != 1 || self.primary_root() != Some(Path::new(&primaries[0].path)) {
            return Err("工作区主文件夹绑定无效。".into());
        }
        for folder in folders {
            if folder.id.trim().is_empty()
                || !valid_alias(&folder.alias)
                || !ids.insert(&folder.id)
                || !aliases.insert(&folder.alias)
                || !Path::new(&folder.path).is_absolute()
                || folder.canonical_path.is_some() != folder.directory_identity.is_some()
            {
                return Err("工作区文件夹身份无效。".into());
            }
            if let Some(identity) = &folder.directory_identity {
                identity.validate().map_err(str::to_string)?;
            }
            if folder
                .canonical_path
                .as_ref()
                .is_some_and(|p| !Path::new(p).is_absolute())
            {
                return Err("工作区文件夹路径无效。".into());
            }
        }
        for (i, folder) in folders.iter().enumerate() {
            if let Some(root) = folder.canonical_path.as_deref() {
                for other in &folders[..i] {
                    if let Some(path) = other.canonical_path.as_deref() {
                        if Path::new(root).starts_with(path) || Path::new(path).starts_with(root) {
                            return Err("工作区文件夹不能重叠。".into());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Checks only the selected root. An offline auxiliary root does not disable other roots.
    pub fn validated_root(&self, folder: &WorkspaceFolder) -> Result<PathBuf, String> {
        self.validate_shape()?;
        let expected = folder
            .canonical_path
            .as_deref()
            .ok_or("工作区文件夹在本轮不可用。")?;
        let current = Path::new(&folder.path)
            .canonicalize()
            .map_err(|_| "工作区文件夹不可用。")?;
        if current != Path::new(expected)
            || Some(FileChangeDirectoryIdentity::read(&current).map_err(str::to_string)?)
                != folder.directory_identity
        {
            return Err("工作区文件夹身份已变化。".into());
        }
        Ok(current)
    }

    pub fn canonical_primary(&self) -> Result<PathBuf, String> {
        self.validate_shape()?;
        if let Some(primary) = self
            .folders()
            .iter()
            .find(|f| f.role == ProjectFolderRole::Primary)
        {
            return self.validated_root(primary);
        }
        self.primary_root()
            .ok_or("此操作需要工作区。")?
            .canonicalize()
            .map_err(|_| "工作区主文件夹不可用。".into())
    }

    /// Discovery uses the frozen primary only. A root that was offline at capture cannot become
    /// a new Skill source halfway through a turn; an available root is revalidated on every use.
    pub fn available_primary(&self) -> Result<Option<PathBuf>, String> {
        self.validate_shape()?;
        if let Some(primary) = self
            .folders()
            .iter()
            .find(|f| f.role == ProjectFolderRole::Primary)
        {
            return if primary.canonical_path.is_none() {
                Ok(None)
            } else {
                self.validated_root(primary).map(Some)
            };
        }
        Ok(self.primary_root().and_then(|p| p.canonicalize().ok()))
    }

    /// Returns an unresolved absolute target. Never canonicalizes the target's leaf.
    pub fn resolve_input(&self, input: &str) -> Result<PathBuf, String> {
        self.validate_shape()?;
        let input = input.trim();
        let locator =
            crate::resource_locator::ResourceLocator::parse(input).map_err(|e| e.to_string())?;
        if let Some((alias, relative)) = parse_workspace_path(input)? {
            let folder = self
                .folders()
                .iter()
                .find(|f| f.alias == alias)
                .ok_or("未知工作区文件夹别名。")?;
            return Ok(self.validated_root(folder)?.join(relative));
        }
        match locator {
            crate::resource_locator::ResourceLocator::Filesystem(_)
            | crate::resource_locator::ResourceLocator::SystemAlias(_) => {}
            _ => return Err("此操作需要文件系统路径。".into()),
        }
        if let Some(expanded) = crate::expand_system_path(input)? {
            self.validate_declared_absolute(&expanded)?;
            return Ok(expanded);
        }
        let path = Path::new(input);
        if path.is_absolute() {
            self.validate_declared_absolute(path)?;
            return Ok(path.to_path_buf());
        }
        Ok(self.canonical_primary()?.join(clean_relative(path)?))
    }

    fn validate_declared_absolute(&self, target: &Path) -> Result<(), String> {
        if target
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return Err("工作区路径不能包含 .. 或越过文件夹边界。".into());
        }
        for folder in self.folders() {
            if target.starts_with(&folder.path)
                || folder
                    .canonical_path
                    .as_deref()
                    .is_some_and(|root| target.starts_with(root))
            {
                self.validated_root(folder)?;
            }
        }
        Ok(())
    }

    /// Membership of an already canonical target (or canonical parent for a new file).
    pub fn containing_root(&self, target: &Path) -> Result<Option<PathBuf>, String> {
        self.validate_shape()?;
        if self.folders().is_empty() {
            return Ok(self
                .primary_root()
                .and_then(|root| root.canonicalize().ok())
                .filter(|root| target.starts_with(root)));
        }
        for folder in self.folders() {
            if folder
                .canonical_path
                .as_deref()
                .is_some_and(|p| target.starts_with(p))
            {
                return self.validated_root(folder).map(Some);
            }
            if target.starts_with(&folder.path) {
                self.validated_root(folder)?;
            }
        }
        Ok(None)
    }

    pub fn contains(&self, target: &Path) -> bool {
        self.containing_root(target).ok().flatten().is_some()
    }

    /// Trusted runtime components may neither live in nor contain a user-editable root.
    pub fn overlaps_root(&self, target: &Path) -> Result<bool, String> {
        self.validate_shape()?;
        if self.folders().is_empty() {
            return Ok(self.primary_root().is_some_and(|root| {
                let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
                root.starts_with(target) || target.starts_with(root)
            }));
        }
        for folder in self.folders() {
            let root = Path::new(folder.canonical_path.as_deref().unwrap_or(&folder.path));
            if root.starts_with(target) || target.starts_with(root) {
                self.validated_root(folder)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn display_path(&self, target: &Path) -> String {
        for folder in self.folders() {
            if let Some(root) = folder.canonical_path.as_deref() {
                if let Ok(relative) = target.strip_prefix(root) {
                    let relative = path_text(relative);
                    return if folder.role == ProjectFolderRole::Primary {
                        escape_relative(&relative)
                    } else if relative.is_empty() {
                        format!("@workspace/{}", folder.alias)
                    } else {
                        format!("@workspace/{}/{}", folder.alias, relative)
                    };
                }
            }
        }
        if self.folders().is_empty() {
            if let Some(root) = self.primary_root().and_then(|p| p.canonicalize().ok()) {
                if let Ok(relative) = target.strip_prefix(root) {
                    return escape_relative(&path_text(relative));
                }
            }
        }
        target.to_string_lossy().into_owned()
    }
}

pub fn capture_project_workspace(project: &ProjectRecord) -> Result<AgentWorkspaceContext, String> {
    let mut folders = Vec::new();
    for folder in &project.folders {
        let captured = Path::new(&folder.path)
            .canonicalize()
            .ok()
            .and_then(|path| {
                FileChangeDirectoryIdentity::read(&path)
                    .ok()
                    .map(|identity| (path, identity))
            });
        let (canonical_path, directory_identity) = match captured {
            Some((path, identity)) => (Some(path.to_string_lossy().into_owned()), Some(identity)),
            None => (None, None),
        };
        folders.push(WorkspaceFolder {
            id: folder.id.clone(),
            alias: folder.alias.clone(),
            role: folder.role,
            path: folder.path.clone(),
            canonical_path,
            directory_identity,
        });
    }
    let context = AgentWorkspaceContext {
        project_id: Some(project.id.clone()),
        display_name: Some(project.name.clone()),
        root_path: project.primary_path().map(str::to_string),
        folders,
    };
    WorkspaceResolver::from_context(Some(&context)).validate_shape()?;
    Ok(context)
}

pub fn parse_workspace_path(input: &str) -> Result<Option<(&str, PathBuf)>, String> {
    if input == "@workspace" {
        return Err("工作区路径缺少文件夹别名。".into());
    }
    let Some(suffix) = input.strip_prefix("@workspace/") else {
        return Ok(None);
    };
    let (alias, relative) = suffix.split_once('/').unwrap_or((suffix, ""));
    if !valid_alias(alias)
        || relative.contains('\\')
        || relative.contains('\0')
        || relative.contains('\n')
        || relative.contains('\r')
    {
        return Err("工作区路径无效。".into());
    }
    Ok(Some((alias, clean_relative(Path::new(relative))?)))
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias != "."
        && alias != ".."
        && alias.trim() == alias
        && alias
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn clean_relative(path: &Path) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return Err("工作区路径不能包含 .. 或越过文件夹边界。".into()),
        }
    }
    Ok(out)
}

fn path_text(path: &Path) -> String {
    path.components()
        .filter_map(|p| match p {
            Component::Normal(p) => Some(p.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub fn escape_relative(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    if path.starts_with('@')
        || !matches!(
            crate::resource_locator::ResourceLocator::parse(path),
            Ok(crate::resource_locator::ResourceLocator::Filesystem(_))
        )
    {
        format!("./{path}")
    } else {
        path.to_string()
    }
}

pub fn freeze_project_workspace(project: &ProjectRecord) -> Result<AgentWorkspaceContext, String> {
    let context = capture_project_workspace(project)?;
    if context.root_path.is_some() {
        WorkspaceResolver::from_context(Some(&context)).canonical_primary()?;
    }
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::ProjectFolderRecord;

    fn fixture() -> (tempfile::TempDir, ProjectRecord) {
        let directory = tempfile::tempdir().unwrap();
        let primary = directory.path().join("app");
        let auxiliary = directory.path().join("docs");
        std::fs::create_dir_all(primary.join("@workspace/文档")).unwrap();
        std::fs::create_dir(&auxiliary).unwrap();
        let mut project =
            ProjectRecord::with_primary_folder("p", "Project", primary.to_string_lossy(), 1);
        project.folders.push(ProjectFolderRecord {
            id: "docs-id".into(),
            alias: "文档".into(),
            role: ProjectFolderRole::Auxiliary,
            path: auxiliary.to_string_lossy().into_owned(),
            sort_order: 1,
            created_at: 1,
        });
        (directory, project)
    }

    #[test]
    fn multi_workspace_addresses_roundtrip_and_default_stays_primary() {
        let (_directory, project) = fixture();
        let context = freeze_project_workspace(&project).unwrap();
        let resolver = WorkspaceResolver::from_context(Some(&context));
        for input in [
            "file.txt",
            "@workspace/文档/file.txt",
            "./@workspace/文档/file.txt",
        ] {
            let target = resolver.resolve_input(input).unwrap();
            assert_eq!(resolver.display_path(&target), input);
        }
        assert_eq!(
            resolver.resolve_input(".").unwrap(),
            resolver.canonical_primary().unwrap()
        );
        for input in [
            "@workspace",
            "@workspace/unknown/a",
            "@workspace/文档/../a",
            "@workspace/文档//etc/passwd",
            "@workspace/文档/a\\b",
            "@unknown/a",
            "../a",
            "skill://secret",
        ] {
            assert!(resolver.resolve_input(input).is_err(), "{input}");
        }
    }

    #[test]
    fn frozen_auxiliary_membership_survives_configuration_changes_but_not_entity_replacement() {
        let (directory, mut project) = fixture();
        let context = freeze_project_workspace(&project).unwrap();
        let resolver = WorkspaceResolver::from_context(Some(&context));
        let original = resolver.resolve_input("@workspace/文档/file.txt").unwrap();
        project.folders[1].path = directory
            .path()
            .join("new-docs")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            resolver.resolve_input("@workspace/文档/file.txt").unwrap(),
            original
        );
        let root = original.parent().unwrap();
        std::fs::rename(root, directory.path().join("old-docs")).unwrap();
        std::fs::create_dir(root).unwrap();
        assert!(resolver.resolve_input("@workspace/文档/file.txt").is_err());
        assert!(resolver.resolve_input("primary.txt").is_ok());
    }

    #[test]
    fn offline_auxiliary_is_frozen_unavailable_and_not_a_primary_failure() {
        let (_directory, project) = fixture();
        std::fs::remove_dir(&project.folders[1].path).unwrap();
        let context = freeze_project_workspace(&project).unwrap();
        let resolver = WorkspaceResolver::from_context(Some(&context));
        assert!(resolver.resolve_input("file.txt").is_ok());
        assert!(resolver.resolve_input("@workspace/文档/file.txt").is_err());
        std::fs::create_dir(&project.folders[1].path).unwrap();
        assert!(resolver.resolve_input("@workspace/文档/file.txt").is_err());
        assert!(resolver
            .resolve_input(
                &Path::new(&project.folders[1].path)
                    .join("file.txt")
                    .to_string_lossy()
            )
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn frozen_root_absolute_and_system_addresses_cannot_follow_a_replacement_symlink() {
        let (directory, project) = fixture();
        let context = freeze_project_workspace(&project).unwrap();
        let resolver = WorkspaceResolver::from_context(Some(&context));
        let root = Path::new(&project.folders[1].path);
        let outside = directory.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("file.txt"), "replacement").unwrap();
        std::fs::remove_dir(root).unwrap();
        std::os::unix::fs::symlink(&outside, root).unwrap();
        assert!(resolver
            .resolve_input(&root.join("file.txt").to_string_lossy())
            .is_err());
        assert!(resolver
            .resolve_input(
                &directory
                    .path()
                    .join("app/../docs/file.txt")
                    .to_string_lossy()
            )
            .is_err());
        assert!(resolver
            .resolve_input(&outside.join("file.txt").to_string_lossy())
            .is_ok());
    }

    #[test]
    fn workspace_model_projection_never_includes_host_paths_or_ids() {
        let (_directory, project) = fixture();
        let context = freeze_project_workspace(&project).unwrap();
        let section = crate::world_state::workspace_binding_section(
            Some(&context),
            crate::WorldStateLifetime::Run,
        )
        .unwrap();
        let value = serde_json::to_value(section).unwrap();
        let rendered = value.to_string();
        assert!(rendered.contains("@workspace/文档"));
        // Projection is tested separately from Host-owned state.
        let projection = value.get("modelProjection").unwrap().to_string();
        assert!(!projection.contains(&project.folders[0].path));
        assert!(!projection.contains("docs-id"));
        assert!(projection.contains("primary_only"));
    }
}
