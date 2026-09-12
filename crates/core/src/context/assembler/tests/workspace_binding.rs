use super::*;
use crate::storage::models::ProjectFolderRole;
use crate::world_state::workspace_binding_section;

fn workspace_snapshot(sequence: u64, docs_source: &str) -> WorldStateSnapshot {
    let folders = [("app", "app"), ("docs", docs_source)]
        .into_iter()
        .map(|(alias, source)| crate::workspace::WorkspaceFolder {
            id: format!("private-folder-{source}"),
            alias: alias.into(),
            role: if alias == "app" {
                ProjectFolderRole::Primary
            } else {
                ProjectFolderRole::Auxiliary
            },
            path: format!("/private/sources/{source}"),
            canonical_path: Some(format!("/private/sources/{source}")),
            directory_identity: Some(crate::file_change::FileChangeDirectoryIdentity::Unix {
                schema_version: 1,
                device: 1,
                inode: 1,
            }),
        })
        .collect();
    let workspace = crate::AgentWorkspaceContext {
        project_id: Some("private-project".into()),
        display_name: Some("project".into()),
        root_path: Some("/private/sources/app".into()),
        folders,
    };
    WorldStateSnapshot::new(
        "workspace-epoch",
        sequence,
        vec![
            workspace_binding_section(Some(&workspace), WorldStateLifetime::Conversation).unwrap(),
        ],
    )
    .unwrap()
}

fn assemble_workspace(records: Vec<AnchoredWorldStateRecord>) -> ContextFrame {
    ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".into(),
        compaction_summary: None,
        world_state_records: records,
        initial_run_world_state: None,
        messages: vec![identified_message("new-input", "user", "read the docs")],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap()
}

#[test]
fn workspace_binding_preview_live_sync_and_cold_restore_render_the_same_source_patch() {
    let initial = workspace_snapshot(0, "docs-old");
    let replaced = workspace_snapshot(1, "docs-new");
    let records = vec![
        AnchoredWorldStateRecord::new(WorldStateRecord::Full(initial.clone()), None).unwrap(),
        AnchoredWorldStateRecord::new(
            WorldStateRecord::Diff(WorldStateDiff::between(&initial, &replaced).unwrap()),
            Some("new-input".into()),
        )
        .unwrap(),
    ];
    let mut live = assemble_workspace(records[..1].to_vec());
    let preview = live
        .conversation_world_state_preview(&replaced.sections)
        .unwrap()
        .expect("same-alias source replacement must be visible before the next run");
    let preview = ContextFrame::new(vec![preview]).to_messages();
    let preview = preview[0].content();
    assert!(preview.contains("\"op\":\"patch\""));
    assert!(preview.contains("source_replaced"));
    assert!(!preview.contains("private-folder"));
    assert!(!preview.contains("/private/sources"));
    assert_eq!(
        live.sync_conversation_world_state_records(&records, false)
            .unwrap(),
        1
    );
    assert_eq!(
        live.sync_conversation_world_state_records(&records, false)
            .unwrap(),
        0
    );
    let cold = assemble_workspace(records.clone());
    assert_eq!(live.to_messages(), cold.to_messages());
    let messages = cold.to_messages();
    let patch = messages
        .iter()
        .find(|message| message.content().contains("\"op\":\"patch\""))
        .expect("cold recovery must reconstruct the same patch");
    assert_eq!(patch.content(), preview);
    assert_eq!(messages.last().unwrap().content(), "read the docs");
    assert!(live
        .conversation_world_state_preview(&replaced.sections)
        .unwrap()
        .is_none());

    // Checkpoint restoration compares the exact already-rendered frame with canonical records.
    live.restore_conversation_world_state_records(records)
        .unwrap();
    assert_eq!(live.to_messages(), cold.to_messages());
}

#[test]
fn workspace_binding_reordered_host_snapshot_advances_authority_without_context_noise() {
    let initial = workspace_snapshot(0, "docs");
    let mut workspace: crate::AgentWorkspaceContext = crate::AgentWorkspaceContext {
        project_id: Some("private-project".into()),
        display_name: Some("project".into()),
        root_path: Some("/private/sources/app".into()),
        folders: serde_json::from_value(initial.sections[0].state["folders"].clone()).unwrap(),
    };
    workspace.folders.reverse();
    let reordered = WorldStateSnapshot::new(
        initial.epoch_id.clone(),
        1,
        vec![
            workspace_binding_section(Some(&workspace), WorldStateLifetime::Conversation).unwrap(),
        ],
    )
    .unwrap();
    let mut records =
        vec![AnchoredWorldStateRecord::new(WorldStateRecord::Full(initial.clone()), None).unwrap()];
    let mut frame = assemble_workspace(records.clone());
    let before = frame.to_messages();
    assert!(frame
        .conversation_world_state_preview(&reordered.sections)
        .unwrap()
        .is_none());
    records.push(
        AnchoredWorldStateRecord::new(
            WorldStateRecord::Diff(WorldStateDiff::between(&initial, &reordered).unwrap()),
            Some("new-input".into()),
        )
        .unwrap(),
    );
    assert_eq!(
        frame
            .sync_conversation_world_state_records(&records, false)
            .unwrap(),
        0
    );
    assert_eq!(frame.conversation_world_state_records(), records);
    assert_eq!(frame.to_messages(), before);
    assert_eq!(assemble_workspace(records.clone()).to_messages(), before);
    frame
        .restore_conversation_world_state_records(records)
        .unwrap();
}
