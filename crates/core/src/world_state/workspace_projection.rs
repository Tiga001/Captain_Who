//! Compact workspace changes for model context. Full Host sections remain the replay authority.

use super::{WorldStateModelChange, WorldStateSectionEnvelope, WorldStateSectionId};
use crate::{storage::models::ProjectFolderRole, workspace::WorkspaceFolder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The only folder fields approved for model context. Internal identity is used for comparison,
/// never copied into this projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceFolderModelProjection {
    pub alias: String,
    pub role: ProjectFolderRole,
    pub available: bool,
    pub path: String,
}

impl From<&WorkspaceFolder> for WorkspaceFolderModelProjection {
    fn from(folder: &WorkspaceFolder) -> Self {
        Self {
            alias: folder.alias.clone(),
            role: folder.role,
            available: folder.canonical_path.is_some(),
            path: format!("@workspace/{}", folder.alias),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceFolderUpdateReason {
    SourceReplaced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceFolderModelChange {
    Added {
        folder: WorkspaceFolderModelProjection,
    },
    Removed {
        alias: String,
    },
    Updated {
        folder: WorkspaceFolderModelProjection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<WorkspaceFolderUpdateReason>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceBindingProjection {
    available: bool,
    display_name: Option<String>,
    path_convention: String,
    folders: Vec<WorkspaceFolderModelProjection>,
    default_scope: String,
}

struct WorkspaceBindingView {
    globals: Map<String, Value>,
    folders: BTreeMap<String, (WorkspaceFolderModelProjection, WorkspaceFolder)>,
}

impl WorkspaceBindingView {
    fn from_section(section: &WorldStateSectionEnvelope) -> Option<Self> {
        let projection: WorkspaceBindingProjection =
            serde_json::from_value(section.model_projection.as_ref()?.clone()).ok()?;
        let host_folders = section.state.get("folders")?;
        let host_folders: Vec<WorkspaceFolder> = if host_folders.is_null() {
            Vec::new()
        } else {
            serde_json::from_value(host_folders.clone()).ok()?
        };
        if host_folders.len() != projection.folders.len() {
            return None;
        }
        let mut folders = BTreeMap::new();
        for host_folder in host_folders {
            // Restrict patches to the canonical builder's projection. Other/custom fixtures keep
            // their existing generic section-replacement behavior instead of silently losing data.
            let folder = projection
                .folders
                .iter()
                .find(|folder| folder.alias == host_folder.alias)?;
            if folder != &WorkspaceFolderModelProjection::from(&host_folder)
                || folders
                    .insert(folder.alias.clone(), (folder.clone(), host_folder))
                    .is_some()
            {
                return None;
            }
        }
        let globals = Map::from_iter([
            ("available".into(), Value::Bool(projection.available)),
            (
                "displayName".into(),
                projection
                    .display_name
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            ),
            (
                "pathConvention".into(),
                Value::String(projection.path_convention),
            ),
            (
                "defaultScope".into(),
                Value::String(projection.default_scope),
            ),
        ]);
        Some(Self { globals, folders })
    }
}

/// `None` means the sections do not support typed workspace patches; `Some(None)` is a valid
/// workspace transition with no model-relevant change, including UI-only folder reordering.
pub(super) fn model_patch(
    before: Option<&WorldStateSectionEnvelope>,
    after: Option<&WorldStateSectionEnvelope>,
) -> Option<Option<WorldStateModelChange>> {
    let before = WorkspaceBindingView::from_section(before?)?;
    let after = WorkspaceBindingView::from_section(after?)?;
    let set = after
        .globals
        .into_iter()
        .filter(|(key, value)| before.globals.get(key) != Some(value))
        .collect::<Map<_, _>>();
    let aliases = before
        .folders
        .keys()
        .chain(after.folders.keys())
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for alias in aliases {
        match (before.folders.get(alias), after.folders.get(alias)) {
            (None, Some((folder, _))) => changes.push(WorkspaceFolderModelChange::Added {
                folder: folder.clone(),
            }),
            (Some(_), None) => changes.push(WorkspaceFolderModelChange::Removed {
                alias: alias.clone(),
            }),
            (Some((old_folder, old_host)), Some((folder, host))) => {
                let replaced = source_replaced(old_host, host);
                if old_folder != folder || replaced {
                    changes.push(WorkspaceFolderModelChange::Updated {
                        folder: folder.clone(),
                        reason: replaced.then_some(WorkspaceFolderUpdateReason::SourceReplaced),
                    });
                }
            }
            _ => {}
        }
    }
    Some(
        (!set.is_empty() || !changes.is_empty()).then_some(WorldStateModelChange::Patch {
            section_id: WorldStateSectionId::WorkspaceBinding,
            set,
            changes,
        }),
    )
}

fn source_replaced(before: &WorkspaceFolder, after: &WorkspaceFolder) -> bool {
    before.id != after.id
        || before.path != after.path
        // None means this capture could not observe the physical directory. Losing/restoring
        // availability alone is not evidence that it was replaced.
        || matches!(
            (&before.canonical_path, &after.canonical_path),
            (Some(before), Some(after)) if before != after
        )
        || matches!(
            (&before.directory_identity, &after.directory_identity),
            (Some(before), Some(after)) if before != after
        )
}

#[cfg(test)]
mod tests;
