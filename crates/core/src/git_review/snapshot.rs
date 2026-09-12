use super::*;

#[derive(Debug, Clone)]
pub(super) struct SnapshotFile {
    pub(super) path: String,
    pub(super) previous_path: Option<String>,
    pub(super) status: GitReviewFileStatus,
    pub(super) stamp: FileStamp,
    pub(super) previous_stamp: Option<FileStamp>,
}

#[derive(Debug, Clone)]
pub(super) struct Snapshot {
    pub(super) id: String,
    pub(super) created_at: Instant,
    pub(super) repository: RepositoryContext,
    pub(super) project_binding: Option<project::ProjectSourceBinding>,
    pub(super) head_oid: String,
    pub(super) target: GitReviewTarget,
    pub(super) comparison: SnapshotComparison,
    pub(super) has_head: bool,
    pub(super) index_stamp: FileStamp,
    pub(super) files: HashMap<String, SnapshotFile>,
}

#[derive(Debug, Clone)]
pub(super) struct TurnSnapshotFile {
    pub(super) path: String,
    pub(super) before: crate::AgentTurnFileContent,
    pub(super) after: crate::AgentTurnFileContent,
}

#[derive(Debug, Clone)]
pub(super) struct TurnSnapshot {
    pub(super) id: String,
    pub(super) created_at: Instant,
    pub(super) files: HashMap<String, TurnSnapshotFile>,
}

#[derive(Default)]
pub(super) struct SnapshotCache {
    entries: HashMap<String, Snapshot>,
    order: VecDeque<String>,
}

#[derive(Default)]
pub(super) struct TurnSnapshotCache {
    entries: HashMap<String, TurnSnapshot>,
    order: VecDeque<String>,
}

impl TurnSnapshotCache {
    pub(super) fn insert(&mut self, snapshot: TurnSnapshot) {
        self.prune();
        while self.order.len() >= MAX_SNAPSHOTS {
            if let Some(id) = self.order.pop_front() {
                self.entries.remove(&id);
            }
        }
        self.order.push_back(snapshot.id.clone());
        self.entries.insert(snapshot.id.clone(), snapshot);
    }

    pub(super) fn get(&mut self, snapshot_id: &str) -> Option<TurnSnapshot> {
        self.prune();
        self.entries.get(snapshot_id).cloned()
    }

    fn prune(&mut self) {
        let expired = self
            .entries
            .iter()
            .filter(|(_, snapshot)| snapshot.created_at.elapsed() > SNAPSHOT_TTL)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            self.entries.remove(&id);
            self.order.retain(|candidate| candidate != &id);
        }
    }
}

impl SnapshotCache {
    pub(super) fn remove(&mut self, id: &str) {
        self.entries.remove(id);
        self.order.retain(|candidate| candidate != id);
    }

    pub(super) fn insert(&mut self, snapshot: Snapshot) {
        self.prune();
        while self.order.len() >= MAX_SNAPSHOTS {
            if let Some(id) = self.order.pop_front() {
                self.entries.remove(&id);
            }
        }
        self.order.push_back(snapshot.id.clone());
        self.entries.insert(snapshot.id.clone(), snapshot);
    }

    pub(super) fn get(&mut self, snapshot_id: &str) -> Option<Snapshot> {
        self.prune();
        self.entries.get(snapshot_id).cloned()
    }

    fn prune(&mut self) {
        let expired = self
            .entries
            .iter()
            .filter(|(_, snapshot)| snapshot.created_at.elapsed() > SNAPSHOT_TTL)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            self.entries.remove(&id);
            self.order.retain(|candidate| candidate != &id);
        }
    }
}

pub(super) fn read_head_oid(repository: &RepositoryContext) -> Result<String, String> {
    let output = run_git(
        &repository.root,
        &[
            "rev-parse".to_string(),
            "--verify".to_string(),
            "HEAD".to_string(),
        ],
        4096,
    )?;
    if output.status.success() {
        String::from_utf8(trim_ascii(&output.stdout).to_vec())
            .map_err(|_| "Git returned an invalid HEAD revision.".to_string())
    } else {
        Ok(String::new())
    }
}

pub(super) fn snapshot_file_is_current(
    snapshot: &Snapshot,
    file: &SnapshotFile,
) -> Result<bool, String> {
    if !repository_is_current(&snapshot.repository) {
        return Ok(false);
    }
    // Unstage uses live HEAD; it may not reinterpret a snapshot after a checkout/reset.
    if snapshot.target.is_mutable() && read_head_oid(&snapshot.repository)? != snapshot.head_oid {
        return Ok(false);
    }
    let previous_path_is_current = match (&file.previous_path, &file.previous_stamp) {
        (Some(path), Some(stamp)) => file_stamp(&snapshot.repository.root.join(path)) == *stamp,
        (None, None) => true,
        _ => false,
    };
    let index_is_current =
        file_stamp(&snapshot.repository.git_dir.join("index")) == snapshot.index_stamp;
    let worktree_is_current = file_stamp(&snapshot.repository.root.join(&file.path)) == file.stamp
        && previous_path_is_current;
    Ok(match &snapshot.comparison {
        SnapshotComparison::IndexToWorktree => index_is_current && worktree_is_current,
        SnapshotComparison::TreeToIndex { .. } => index_is_current,
        SnapshotComparison::TreeToWorktree { .. } => index_is_current && worktree_is_current,
        SnapshotComparison::TreeToTree { .. } => true,
    })
}
