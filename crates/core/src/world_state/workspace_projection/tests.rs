use super::*;
use crate::{
    file_change::FileChangeDirectoryIdentity,
    protocol::AgentWorkspaceContext,
    world_state::{
        workspace_binding_section, WorldStateDiff, WorldStateLifetime, WorldStateModelRecord,
        WorldStateOperation, WorldStateReducer, WorldStateSnapshot,
    },
};
use serde_json::json;

fn folder(alias: &str, role: ProjectFolderRole) -> WorkspaceFolder {
    WorkspaceFolder {
        id: format!("private-folder-id-{alias}"),
        alias: alias.into(),
        role,
        path: format!("/private/source/{alias}"),
        canonical_path: Some(format!("/private/canonical/{alias}")),
        directory_identity: Some(FileChangeDirectoryIdentity::Unix {
            schema_version: 1,
            device: 7,
            inode: 11,
        }),
    }
}

fn workspace(folders: Vec<WorkspaceFolder>) -> AgentWorkspaceContext {
    AgentWorkspaceContext {
        project_id: Some("private-project-id".into()),
        display_name: Some("Project".into()),
        root_path: folders
            .iter()
            .find(|folder| folder.role == ProjectFolderRole::Primary)
            .map(|folder| folder.path.clone()),
        folders,
    }
}

fn snapshot(workspace: Option<&AgentWorkspaceContext>, sequence: u64) -> WorldStateSnapshot {
    WorldStateSnapshot::new(
        "workspace-epoch",
        sequence,
        vec![workspace_binding_section(workspace, WorldStateLifetime::Conversation).unwrap()],
    )
    .unwrap()
}

fn model_diff(before: &WorldStateSnapshot, after: &WorldStateSnapshot) -> Option<Value> {
    let diff = WorldStateDiff::between(before, after).unwrap();
    assert_eq!(
        WorldStateReducer::fold(before.clone(), std::slice::from_ref(&diff)).unwrap(),
        *after,
        "the durable diff still reconstructs the exact complete Host state"
    );
    diff.model_projection_against(before, WorldStateLifetime::Conversation)
        .unwrap()
        .map(|record| {
            let encoded = serde_json::to_value(&record).unwrap();
            assert_eq!(
                serde_json::from_value::<WorldStateModelRecord>(encoded.clone()).unwrap(),
                record
            );
            let text = record.render_sanitized_text();
            for secret in [
                "/private/",
                "private-folder-id",
                "private-project-id",
                "canonicalPath",
                "directoryIdentity",
                "world-state-sha256",
                "workspace-epoch",
            ] {
                assert!(!text.contains(secret), "projection leaked {secret}");
            }
            encoded
        })
}

fn projected_folder(folder: &WorkspaceFolder) -> Value {
    serde_json::to_value(WorkspaceFolderModelProjection::from(folder)).unwrap()
}

/// Applies the public patch semantics, independently from the Host reducer, to ensure the compact
/// context retains the same complete folder map/global values as a fresh full baseline.
fn apply_model_patch(mut baseline: Value, patch: &Value) -> Value {
    assert_eq!(patch["op"], "patch");
    assert_eq!(patch["sectionId"], "workspace.binding");
    if let Some(set) = patch.get("set") {
        baseline
            .as_object_mut()
            .unwrap()
            .extend(set.as_object().unwrap().clone());
    }
    let mut folders: BTreeMap<String, Value> = baseline["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|folder| {
            (
                folder["alias"].as_str().unwrap().to_string(),
                folder.clone(),
            )
        })
        .collect();
    for change in patch["changes"].as_array().unwrap() {
        match change["kind"].as_str().unwrap() {
            "removed" => {
                assert!(folders.remove(change["alias"].as_str().unwrap()).is_some());
            }
            "added" => {
                assert!(folders
                    .insert(
                        change["folder"]["alias"].as_str().unwrap().to_string(),
                        change["folder"].clone()
                    )
                    .is_none());
            }
            "updated" => {
                assert!(folders
                    .insert(
                        change["folder"]["alias"].as_str().unwrap().to_string(),
                        change["folder"].clone()
                    )
                    .is_some());
            }
            kind => panic!("unknown change {kind}"),
        }
    }
    baseline["folders"] = Value::Array(folders.into_values().collect());
    baseline
}

fn projection(snapshot: &WorldStateSnapshot) -> Value {
    snapshot.sections[0].model_projection.clone().unwrap()
}

#[test]
fn full_workspace_baseline_is_complete_canonical_and_sanitized() {
    let zed = folder("zed", ProjectFolderRole::Primary);
    let api = folder("api", ProjectFolderRole::Auxiliary);
    let context = workspace(vec![zed.clone(), api.clone()]);
    let baseline = snapshot(Some(&context), 0);
    let full = baseline
        .model_projection(WorldStateLifetime::Conversation)
        .unwrap();
    let encoded = serde_json::to_value(&full).unwrap();
    assert_eq!(encoded["recordType"], "full");
    assert_eq!(
        encoded["sections"][0]["value"]["folders"],
        json!([projected_folder(&api), projected_folder(&zed)])
    );
    assert_eq!(baseline.sections[0].state["folders"][0]["alias"], "zed");
    assert!(!full.render_sanitized_text().contains("/private/"));
    assert!(!full.render_sanitized_text().contains("private-folder-id"));
}

#[test]
fn folder_patch_sends_only_net_additions_removals_and_role_changes() {
    let app = folder("app", ProjectFolderRole::Primary);
    let api = folder("api", ProjectFolderRole::Auxiliary);
    let docs = folder("docs", ProjectFolderRole::Auxiliary);
    let shared = folder("shared", ProjectFolderRole::Auxiliary);
    let mut after_app = app.clone();
    after_app.role = ProjectFolderRole::Auxiliary;
    let mut after_api = api.clone();
    after_api.role = ProjectFolderRole::Primary;
    let web = folder("web", ProjectFolderRole::Auxiliary);
    let before = snapshot(Some(&workspace(vec![app, api, docs, shared.clone()])), 0);
    let after = snapshot(
        Some(&workspace(vec![
            after_api.clone(),
            shared,
            web.clone(),
            after_app.clone(),
        ])),
        1,
    );
    let diff = WorldStateDiff::between(&before, &after).unwrap();
    assert!(matches!(
        diff.operations.as_slice(),
        [WorldStateOperation::Replace { .. }]
    ));
    let record = model_diff(&before, &after).unwrap();
    assert_eq!(
        record["changes"],
        json!([{
            "op":"patch", "sectionId":"workspace.binding",
            "changes":[
                {"kind":"updated","folder":projected_folder(&after_api)},
                {"kind":"updated","folder":projected_folder(&after_app)},
                {"kind":"removed","alias":"docs"},
                {"kind":"added","folder":projected_folder(&web)}
            ]
        }])
    );
    assert_eq!(
        apply_model_patch(projection(&before), &record["changes"][0]),
        projection(&after)
    );
}

#[test]
fn same_alias_source_replacement_is_visible_without_exposing_identity() {
    let app = folder("app", ProjectFolderRole::Primary);
    let before = snapshot(Some(&workspace(vec![app.clone()])), 0);
    let mut replacements = Vec::new();
    let mut changed = app.clone();
    changed.id = "private-folder-id-replacement".into();
    replacements.push(changed);
    let mut changed = app.clone();
    changed.path = "/private/repointed/app".into();
    replacements.push(changed);
    let mut changed = app.clone();
    changed.canonical_path = Some("/private/repointed-canonical/app".into());
    replacements.push(changed);
    let mut changed = app.clone();
    changed.directory_identity = Some(FileChangeDirectoryIdentity::Unix {
        schema_version: 1,
        device: 7,
        inode: 12,
    });
    replacements.push(changed);
    for replacement in replacements {
        let after = snapshot(Some(&workspace(vec![replacement])), 1);
        assert_eq!(projection(&before), projection(&after));
        let record = model_diff(&before, &after).unwrap();
        assert_eq!(
            record["changes"][0],
            json!({
                "op":"patch", "sectionId":"workspace.binding",
                "changes":[{
                    "kind":"updated", "folder":projected_folder(&app),
                    "reason":"source_replaced"
                }]
            })
        );
    }
}

#[test]
fn availability_round_trip_updates_globals_without_false_source_replacement() {
    let app = folder("app", ProjectFolderRole::Primary);
    let mut offline = app.clone();
    offline.canonical_path = None;
    offline.directory_identity = None;
    let online = snapshot(Some(&workspace(vec![app.clone()])), 0);
    let unavailable = snapshot(Some(&workspace(vec![offline.clone()])), 1);
    let restored = snapshot(Some(&workspace(vec![app.clone()])), 2);
    let down = model_diff(&online, &unavailable).unwrap();
    let up = model_diff(&unavailable, &restored).unwrap();
    assert_eq!(
        down["changes"][0],
        json!({
            "op":"patch", "sectionId":"workspace.binding",
            "set":{"available":false,"pathConvention":"no_workspace","defaultScope":"no_workspace"},
            "changes":[{"kind":"updated","folder":projected_folder(&offline)}]
        })
    );
    assert_eq!(
        up["changes"][0],
        json!({
            "op":"patch", "sectionId":"workspace.binding",
            "set":{"available":true,"pathConvention":"workspace_relative","defaultScope":"primary_only"},
            "changes":[{"kind":"updated","folder":projected_folder(&app)}]
        })
    );
    assert_eq!(
        apply_model_patch(
            apply_model_patch(projection(&online), &down["changes"][0]),
            &up["changes"][0]
        ),
        projection(&restored)
    );
}

#[test]
fn workspace_attach_detach_and_nullable_display_name_preserve_complete_state() {
    let context = workspace(vec![folder("app", ProjectFolderRole::Primary)]);
    let none = snapshot(None, 0);
    let attached = snapshot(Some(&context), 1);
    let mut anonymous = context.clone();
    anonymous.display_name = None;
    let unnamed = snapshot(Some(&anonymous), 2);
    let detached = snapshot(None, 3);
    let mut reconstructed = projection(&none);
    for (before, after) in [
        (&none, &attached),
        (&attached, &unnamed),
        (&unnamed, &detached),
    ] {
        let diff = model_diff(before, after).unwrap();
        reconstructed = apply_model_patch(reconstructed, &diff["changes"][0]);
        assert_eq!(reconstructed, projection(after));
    }
    assert_eq!(
        model_diff(&attached, &unnamed).unwrap()["changes"][0],
        json!({
            "op":"patch", "sectionId":"workspace.binding",
            "set":{"displayName":null}, "changes":[]
        })
    );
}

#[test]
fn pure_order_or_unrelated_host_changes_do_not_emit_model_patches() {
    let mut context = workspace(vec![
        folder("app", ProjectFolderRole::Primary),
        folder("api", ProjectFolderRole::Auxiliary),
    ]);
    let before = snapshot(Some(&context), 0);
    context.folders.reverse();
    context.project_id = Some("private-project-id-new".into());
    let after = snapshot(Some(&context), 1);
    assert_ne!(before.revision, after.revision);
    assert_eq!(model_diff(&before, &after), None);
}

#[test]
fn unknown_workspace_projection_fields_fall_back_to_complete_replacement() {
    let context = workspace(vec![folder("app", ProjectFolderRole::Primary)]);
    let before = snapshot(Some(&context), 0);
    let mut extended = projection(&before);
    extended["futureProperty"] = json!("preserve this field");
    let section = WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::WorkspaceBinding,
        WorldStateLifetime::Conversation,
        before.sections[0].state.clone(),
        extended.clone(),
    )
    .unwrap();
    let after = WorldStateSnapshot::new("workspace-epoch", 1, vec![section]).unwrap();
    assert_eq!(
        model_diff(&before, &after).unwrap()["changes"][0],
        json!({"op":"replace","sectionId":"workspace.binding","value":extended})
    );
}

#[test]
fn typed_workspace_section_addition_and_removal_keep_generic_semantics() {
    let context = workspace(vec![folder("app", ProjectFolderRole::Primary)]);
    let empty = WorldStateSnapshot::new("workspace-epoch", 0, vec![]).unwrap();
    let workspace = snapshot(Some(&context), 1);
    let empty_again = WorldStateSnapshot::new("workspace-epoch", 2, vec![]).unwrap();
    assert_eq!(
        model_diff(&empty, &workspace).unwrap()["changes"][0],
        json!({"op":"add","sectionId":"workspace.binding","value":projection(&workspace)})
    );
    assert_eq!(
        model_diff(&workspace, &empty_again).unwrap()["changes"][0],
        json!({"op":"remove","sectionId":"workspace.binding"})
    );
}
