use super::*;

pub(super) fn diff_captures(
    before: &CommandArtifactCapture,
    after: &CommandArtifactCapture,
    workspace_root: Option<&Path>,
) -> Vec<AgentCommandArtifactChange> {
    let mut changes = Vec::new();
    let mut deleted = Vec::new();
    let mut created = Vec::new();

    for (path, old) in &before.files {
        if let Some(new) = after.files.get(path) {
            if let Some(kind) = same_path_change_kind(old, new) {
                changes.push(build_change(
                    kind,
                    path,
                    None,
                    old.kind,
                    Some(old.metadata.clone()),
                    Some(new.metadata.clone()),
                    workspace_root,
                ));
            }
        } else if path_is_covered(path, &after.complete_roots, &after.excluded_roots) {
            // Absence is evidence only when the opposite snapshot fully covered this path. A
            // time/entry/hash budget, cancellation, directory race, or read failure must never be
            // converted into a fabricated deletion.
            deleted.push((path, old));
        }
    }
    for (path, new) in &after.files {
        if !before.files.contains_key(path)
            && path_is_covered(path, &before.complete_roots, &before.excluded_roots)
        {
            // Symmetrically, a file seen only after the command is a creation only when the before
            // snapshot proved the path absent. Rename pairing is built solely from these qualified
            // creation/deletion candidates.
            created.push((path, new));
        }
    }

    let mut deleted_by_digest: StdHashMap<(AgentCommandArtifactKind, String, u64), Vec<usize>> =
        StdHashMap::new();
    let mut created_by_digest: StdHashMap<(AgentCommandArtifactKind, String, u64), Vec<usize>> =
        StdHashMap::new();
    for (index, (_, artifact)) in deleted.iter().enumerate() {
        if let Some(digest) = artifact.metadata.sha256.as_ref() {
            deleted_by_digest
                .entry((artifact.kind, digest.clone(), artifact.metadata.size_bytes))
                .or_default()
                .push(index);
        }
    }
    for (index, (_, artifact)) in created.iter().enumerate() {
        if let Some(digest) = artifact.metadata.sha256.as_ref() {
            created_by_digest
                .entry((artifact.kind, digest.clone(), artifact.metadata.size_bytes))
                .or_default()
                .push(index);
        }
    }

    let mut renamed_deleted = HashSet::new();
    let mut renamed_created = HashSet::new();
    for (key, deleted_indexes) in &deleted_by_digest {
        let Some(created_indexes) = created_by_digest.get(key) else {
            continue;
        };
        if deleted_indexes.len() == 1 && created_indexes.len() == 1 {
            let deleted_index = deleted_indexes[0];
            let created_index = created_indexes[0];
            let (old_path, old) = deleted[deleted_index];
            let (new_path, new) = created[created_index];
            changes.push(build_change(
                AgentCommandArtifactChangeKind::Renamed,
                new_path,
                Some(old_path),
                new.kind,
                Some(old.metadata.clone()),
                Some(new.metadata.clone()),
                workspace_root,
            ));
            renamed_deleted.insert(deleted_index);
            renamed_created.insert(created_index);
        }
    }

    for (index, (path, artifact)) in deleted.into_iter().enumerate() {
        if !renamed_deleted.contains(&index) {
            changes.push(build_change(
                AgentCommandArtifactChangeKind::Deleted,
                path,
                None,
                artifact.kind,
                Some(artifact.metadata.clone()),
                None,
                workspace_root,
            ));
        }
    }
    for (index, (path, artifact)) in created.into_iter().enumerate() {
        if !renamed_created.contains(&index) {
            changes.push(build_change(
                AgentCommandArtifactChangeKind::Created,
                path,
                None,
                artifact.kind,
                None,
                Some(artifact.metadata.clone()),
                workspace_root,
            ));
        }
    }
    changes.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| change_kind_order(left.kind).cmp(&change_kind_order(right.kind)))
    });
    changes
}

#[cfg(test)]
pub(super) fn limit_reported_changes(
    changes: Vec<AgentCommandArtifactChange>,
    expected_outputs: &[ExpectedOutput],
    workspace_root: Option<&Path>,
) -> (Vec<AgentCommandArtifactChange>, u64) {
    let expected_paths = expected_outputs
        .iter()
        .filter_map(|expected| expected.resolved_path.as_deref())
        .map(|path| display_observed_path(path, workspace_root))
        .collect::<Vec<_>>();
    limit_projected_changes(changes, &expected_paths)
}

pub(super) fn limit_projected_changes(
    mut changes: Vec<AgentCommandArtifactChange>,
    expected_paths: &[(String, AgentCommandArtifactScope)],
) -> (Vec<AgentCommandArtifactChange>, u64) {
    // Stable sorting keeps the existing deterministic path order within both groups while
    // attempting explicit expected-output changes before incidental workspace changes.
    changes.sort_by_key(|change| !change_matches_expected_output(change, expected_paths));

    let mut reported = Vec::with_capacity(changes.len().min(MAX_REPORTED_CHANGES));
    let mut serialized_bytes = 2_usize; // JSON array brackets.
    let mut omitted = 0_u64;
    for change in changes {
        let encoded_len = serde_json::to_vec(&change)
            .map(|encoded| encoded.len())
            .unwrap_or(MAX_REPORTED_CHANGE_BYTES.saturating_add(1));
        let separator_len = usize::from(!reported.is_empty());
        let fits = reported.len() < MAX_REPORTED_CHANGES
            && serialized_bytes
                .saturating_add(separator_len)
                .saturating_add(encoded_len)
                <= MAX_REPORTED_CHANGE_BYTES;
        if fits {
            serialized_bytes += separator_len + encoded_len;
            reported.push(change);
        } else {
            omitted += 1;
        }
    }
    reported.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| change_kind_order(left.kind).cmp(&change_kind_order(right.kind)))
    });
    (reported, omitted)
}

fn change_matches_expected_output(
    change: &AgentCommandArtifactChange,
    expected_paths: &[(String, AgentCommandArtifactScope)],
) -> bool {
    expected_paths.iter().any(|(path, scope)| {
        (&change.path == path && &change.scope == scope)
            || (change.previous_path.as_ref() == Some(path)
                && change.previous_scope.as_ref() == Some(scope))
    })
}

fn same_path_change_kind(
    before: &ObservedArtifact,
    after: &ObservedArtifact,
) -> Option<AgentCommandArtifactChangeKind> {
    if before.identity.is_some() && after.identity.is_some() && before.identity != after.identity {
        return Some(AgentCommandArtifactChangeKind::Replaced);
    }
    match (
        before.metadata.sha256.as_ref(),
        after.metadata.sha256.as_ref(),
    ) {
        (Some(before_digest), Some(after_digest)) if before_digest != after_digest => {
            Some(AgentCommandArtifactChangeKind::Modified)
        }
        (Some(_), Some(_)) => None,
        _ if before.metadata.size_bytes != after.metadata.size_bytes
            || before.modified_ns != after.modified_ns =>
        {
            Some(AgentCommandArtifactChangeKind::Modified)
        }
        _ => None,
    }
}

fn build_change(
    kind: AgentCommandArtifactChangeKind,
    path: &Path,
    previous_path: Option<&Path>,
    artifact_kind: AgentCommandArtifactKind,
    before: Option<AgentCommandArtifactMetadata>,
    after: Option<AgentCommandArtifactMetadata>,
    workspace_root: Option<&Path>,
) -> AgentCommandArtifactChange {
    let (path, scope) = display_observed_path(path, workspace_root);
    let (previous_path, previous_scope) = previous_path
        .map(|path| display_observed_path(path, workspace_root))
        .map_or((None, None), |(path, scope)| (Some(path), Some(scope)));
    AgentCommandArtifactChange {
        kind,
        artifact_kind,
        path,
        scope,
        previous_path,
        previous_scope,
        before,
        after,
    }
}

pub(super) fn display_observed_path(
    path: &Path,
    workspace_root: Option<&Path>,
) -> (String, AgentCommandArtifactScope) {
    if let Some(relative) = workspace_root.and_then(|root| path.strip_prefix(root).ok()) {
        (display_path(relative), AgentCommandArtifactScope::Workspace)
    } else {
        (display_path(path), AgentCommandArtifactScope::External)
    }
}

pub(super) fn display_path(path: &Path) -> String {
    let parts = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if path.is_absolute() {
        format!("/{}", parts.join("/"))
    } else if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}
