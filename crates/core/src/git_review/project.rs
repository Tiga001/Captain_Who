use super::*;

#[derive(Debug, Clone)]
pub(super) struct ProjectSourceBinding {
    pub(super) project_id: String,
    pub(super) folder: ProjectFolderRecord,
}

pub(super) fn folder_workspace_path(folder: &ProjectFolderRecord, relative: &str) -> String {
    if folder.role == ProjectFolderRole::Auxiliary {
        format!("@workspace/{}/{}", folder.alias, relative)
    } else if relative == "@workspace" || relative.starts_with("@workspace/") {
        format!("./{relative}")
    } else {
        relative.to_string()
    }
}

impl GitReviewService {
    pub fn inspect_project(&self, project: &ProjectRecord) -> GitRepositoryInspection {
        let folders = project
            .folders
            .iter()
            .map(|folder| {
                let inspection = self.inspect_repository(&project.id, Path::new(&folder.path));
                GitRepositorySourceInspection {
                    folder_id: folder.id.clone(),
                    alias: folder.alias.clone(),
                    role: folder.role,
                    state: inspection.state,
                    repository_id: inspection.repository_id,
                    message: inspection.message,
                }
            })
            .collect::<Vec<_>>();
        let default = folders
            .iter()
            .find(|f| {
                f.role == ProjectFolderRole::Primary
                    && f.state == GitRepositoryInspectionState::Ready
            })
            .or_else(|| {
                folders
                    .iter()
                    .find(|f| f.state == GitRepositoryInspectionState::Ready)
            });
        let default_folder_id = default.map(|f| f.folder_id.clone());
        let repository_id = default.map(|_| {
            content_revision(
                serde_json::to_string(&folders)
                    .expect("Git source inspection is serializable")
                    .as_bytes(),
            )
        });
        let state = if default.is_some() {
            GitRepositoryInspectionState::Ready
        } else if folders
            .iter()
            .any(|f| f.state == GitRepositoryInspectionState::Unavailable)
        {
            GitRepositoryInspectionState::Unavailable
        } else if folders
            .iter()
            .any(|f| f.state == GitRepositoryInspectionState::Unsupported)
        {
            GitRepositoryInspectionState::Unsupported
        } else {
            GitRepositoryInspectionState::NotRepository
        };
        GitRepositoryInspection {
            project_id: project.id.clone(),
            state,
            repository_id,
            message: (default_folder_id.is_none())
                .then(|| "No available source folder has a reviewable Git worktree.".to_string()),
            folders,
            default_folder_id,
        }
    }

    pub fn project_source<'a>(
        &self,
        project: &'a ProjectRecord,
        folder_id: Option<&str>,
    ) -> Result<&'a ProjectFolderRecord, String> {
        let default;
        let folder_id = match folder_id {
            Some(value) if !value.trim().is_empty() => value,
            Some(_) => return Err("The selected source folder identity is invalid.".into()),
            None => {
                default = self
                    .inspect_project(project)
                    .default_folder_id
                    .ok_or("No available source folder has a reviewable Git worktree.")?;
                &default
            }
        };
        project
            .folders
            .iter()
            .find(|f| f.id == folder_id)
            .ok_or_else(|| "The selected source folder no longer belongs to this project.".into())
    }

    pub fn review_project_summary(
        &self,
        project: &ProjectRecord,
        folder_id: Option<&str>,
        target: GitReviewTarget,
    ) -> Result<GitReviewSummary, String> {
        let folder = self.project_source(project, folder_id)?;
        self.review_bound_summary(
            Path::new(&folder.path),
            target,
            Some(ProjectSourceBinding {
                project_id: project.id.clone(),
                folder: folder.clone(),
            }),
        )
    }

    pub fn review_project_repository_context(
        &self,
        project: &ProjectRecord,
        folder_id: Option<&str>,
    ) -> Result<GitReviewRepositoryContext, String> {
        let folder = self.project_source(project, folder_id)?;
        self.review_repository_context(Path::new(&folder.path))
    }

    pub fn review_project_commits(
        &self,
        project: &ProjectRecord,
        folder_id: Option<&str>,
    ) -> Result<GitReviewCommitList, String> {
        let folder = self.project_source(project, folder_id)?;
        self.review_commits(Path::new(&folder.path))
    }

    /// Expire ordinary review snapshots whose registered source was removed or rebound.
    /// Last-turn snapshots contain immutable historical text and never authorize mutations.
    pub fn revalidate_snapshot_membership(
        &self,
        snapshot_id: &str,
        projects: &[ProjectRecord],
    ) -> Result<(), String> {
        let mut snapshots = self
            .snapshots
            .lock()
            .map_err(|_| "Git review snapshot cache is unavailable.")?;
        let Some(snapshot) = snapshots.get(snapshot_id) else {
            return Ok(());
        };
        let Some(binding) = snapshot.project_binding.as_ref() else {
            return Ok(());
        };
        let valid = projects
            .iter()
            .find(|p| p.id == binding.project_id)
            .and_then(|p| p.folders.iter().find(|f| f.id == binding.folder.id))
            .is_some_and(|f| {
                f.path == binding.folder.path
                    && f.alias == binding.folder.alias
                    && f.role == binding.folder.role
                    && f.created_at == binding.folder.created_at
            });
        if !valid {
            snapshots.remove(snapshot_id);
        }
        Ok(())
    }
}
