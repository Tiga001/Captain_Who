use serde::{Deserialize, Serialize};

/// Role of one folder inside a project: exactly one primary root plus any auxiliary roots.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProjectFolderRole {
    Primary,
    Auxiliary,
}

impl ProjectFolderRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Auxiliary => "auxiliary",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "primary" => Some(Self::Primary),
            "auxiliary" => Some(Self::Auxiliary),
            _ => None,
        }
    }
}

/// One filesystem root that belongs to a project.
///
/// `alias` is the stable, project-unique name used to address the folder (for example in
/// `@workspace/<alias>/...` references); it is assigned once when the folder is added.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFolderRecord {
    pub id: String,
    pub path: String,
    pub alias: String,
    pub role: ProjectFolderRole,
    pub sort_order: i64,
    pub created_at: i64,
}

/// Maximum number of folders a single project may hold.
pub const MAX_PROJECT_FOLDERS: usize = 32;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    /// Every root of the project in display order. Empty for projects without a workspace;
    /// otherwise exactly one folder has the `primary` role.
    pub folders: Vec<ProjectFolderRecord>,
    pub created_at: i64,
    pub pinned_at: Option<i64>,
}

impl ProjectRecord {
    /// A project that has no workspace folder yet.
    pub fn without_folders(
        id: impl Into<String>,
        name: impl Into<String>,
        created_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            folders: Vec::new(),
            created_at,
            pinned_at: None,
        }
    }

    /// A project whose only root is the primary folder at `path`.
    pub fn with_primary_folder(
        id: impl Into<String>,
        name: impl Into<String>,
        path: impl Into<String>,
        created_at: i64,
    ) -> Self {
        let id = id.into();
        let path = path.into();
        let folder = ProjectFolderRecord {
            id: format!("{id}-primary"),
            alias: project_folder_alias_from_path(&path),
            path,
            role: ProjectFolderRole::Primary,
            sort_order: 0,
            created_at,
        };
        Self {
            id,
            name: name.into(),
            folders: vec![folder],
            created_at,
            pinned_at: None,
        }
    }

    pub fn primary_folder(&self) -> Option<&ProjectFolderRecord> {
        self.folders
            .iter()
            .find(|folder| folder.role == ProjectFolderRole::Primary)
    }

    /// Filesystem path of the primary folder, which stays the working directory of the project.
    pub fn primary_path(&self) -> Option<&str> {
        self.primary_folder().map(|folder| folder.path.as_str())
    }

    pub fn auxiliary_folders(&self) -> impl Iterator<Item = &ProjectFolderRecord> {
        self.folders
            .iter()
            .filter(|folder| folder.role == ProjectFolderRole::Auxiliary)
    }

    /// Structural validation shared by every writer: at most one primary folder (required as
    /// soon as any folder exists), unique ids, paths and aliases, and no blank identifiers.
    pub fn validate_folders(&self) -> Result<(), String> {
        if self.folders.len() > MAX_PROJECT_FOLDERS {
            return Err(format!(
                "项目最多只能包含 {MAX_PROJECT_FOLDERS} 个文件夹：{}",
                self.id
            ));
        }
        let mut primary_count = 0usize;
        let mut ids = std::collections::HashSet::new();
        let mut paths = std::collections::HashSet::new();
        let mut aliases = std::collections::HashSet::new();
        for folder in &self.folders {
            if folder.id.trim().is_empty() || folder.id.trim() != folder.id {
                return Err(format!("项目文件夹标识无效：{}", self.id));
            }
            if folder.path.trim().is_empty() || folder.path.trim() != folder.path {
                return Err(format!("项目文件夹路径不能为空：{}", self.id));
            }
            if folder.alias.trim().is_empty() || folder.alias.trim() != folder.alias {
                return Err(format!("项目文件夹别名不能为空：{}", self.id));
            }
            if folder.sort_order < 0 || folder.created_at < 0 {
                return Err(format!("项目文件夹排序或时间无效：{}", self.id));
            }
            if !ids.insert(folder.id.as_str()) {
                return Err(format!("项目文件夹标识重复：{}", folder.id));
            }
            if !paths.insert(folder.path.as_str()) {
                return Err(format!("项目文件夹路径重复：{}", folder.path));
            }
            if !aliases.insert(folder.alias.as_str()) {
                return Err(format!("项目文件夹别名重复：{}", folder.alias));
            }
            if folder.role == ProjectFolderRole::Primary {
                primary_count += 1;
            }
        }
        match (self.folders.is_empty(), primary_count) {
            (true, _) | (false, 1) => Ok(()),
            (false, 0) => Err(format!("项目必须指定一个主文件夹：{}", self.id)),
            (false, _) => Err(format!("项目只能有一个主文件夹：{}", self.id)),
        }
    }
}

/// Derives a display alias from the final path component, falling back to `workspace`.
pub fn project_folder_alias_from_path(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let base = trimmed
        .rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .unwrap_or_default();
    if base.is_empty() {
        "workspace".to_string()
    } else {
        base.to_string()
    }
}
