use super::*;
use crate::storage::models::ProjectFolderRole;

fn workspace_snapshot(sequence: u64, auxiliary: (&str, &str)) -> WorldStateSnapshot {
    let folders = [("app", "app"), auxiliary]
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
        vec![crate::world_state::workspace_binding_section(
            Some(&workspace),
            WorldStateLifetime::Conversation,
        )
        .unwrap()],
    )
    .unwrap()
}

fn projected_journal(connection: &Connection) -> Vec<String> {
    let records = list_active_journal_entries(connection, "conversation")
        .unwrap()
        .into_iter()
        .map(|entry| entry.anchored_record())
        .collect::<Vec<_>>();
    let WorldStateRecord::Full(initial) = &records[0].record else {
        panic!("missing full baseline")
    };
    let mut reducer = WorldStateReducer::new(initial.clone()).unwrap();
    let mut projected = vec![initial
        .model_projection(WorldStateLifetime::Conversation)
        .unwrap()
        .render_sanitized_text()];
    for record in &records[1..] {
        let WorldStateRecord::Diff(diff) = &record.record else {
            panic!("duplicate full baseline")
        };
        if let Some(projection) = diff
            .model_projection_against(reducer.snapshot(), WorldStateLifetime::Conversation)
            .unwrap()
        {
            projected.push(projection.render_sanitized_text());
        }
        reducer.apply(diff).unwrap();
    }
    projected
}

#[test]
fn workspace_binding_compaction_rebases_a_complete_folder_map_and_keeps_tail_patch() {
    let mut connection = connection();
    insert_conversation(&connection, "conversation");
    insert_message(&connection, "conversation", "message-1", 1);
    insert_message(&connection, "conversation", "message-2", 2);
    let initial = workspace_snapshot(0, ("docs", "docs-old"));
    let at_cutoff = workspace_snapshot(1, ("docs", "docs-new"));
    let current = workspace_snapshot(2, ("website", "site"));
    for (record, anchor, created_at) in [
        (WorldStateRecord::Full(initial.clone()), None, 10),
        (
            WorldStateRecord::Diff(WorldStateDiff::between(&initial, &at_cutoff).unwrap()),
            Some("message-1"),
            11,
        ),
        (
            WorldStateRecord::Diff(WorldStateDiff::between(&at_cutoff, &current).unwrap()),
            Some("message-2"),
            12,
        ),
    ] {
        append(
            &mut connection,
            "conversation",
            1,
            anchor,
            &record,
            created_at,
        )
        .unwrap();
    }
    let before = projected_journal(&connection);
    assert!(before[1].contains("source_replaced"));
    let tail_patch = before[2].clone();

    insert_summary(
        &connection,
        "conversation",
        "message-1",
        "workspace-summary",
    );
    let outcome = rebase_active_epoch(
        &mut connection,
        &ConversationWorldStateRebaseRequest {
            conversation_id: "conversation",
            expected_source_epoch_id: "workspace-epoch",
            expected_source_revision: &current.revision,
            covered_through: &crate::ContextJournalCursor::message("message-1"),
            new_epoch_id: "workspace-rebased",
            base_summary_id: "workspace-summary",
            created_at: 20,
        },
    )
    .unwrap();
    assert_eq!(outcome.full_snapshot.sections, at_cutoff.sections);
    assert_eq!(outcome.head_snapshot.sections, current.sections);
    let after = projected_journal(&connection);
    assert_eq!(after.len(), 2);
    assert_eq!(
        after[0],
        at_cutoff
            .model_projection(WorldStateLifetime::Conversation)
            .unwrap()
            .render_sanitized_text()
    );
    assert!(after[0].contains("\"folders\":["));
    assert!(after[0].contains("@workspace/app"));
    assert!(after[0].contains("@workspace/docs"));
    assert!(!after[0].contains("source_replaced"));
    assert_eq!(after[1], tail_patch);
    assert!(after[1].contains("\"kind\":\"removed\""));
    assert!(after[1].contains("\"kind\":\"added\""));
    assert!(after
        .iter()
        .all(|text| !text.contains("/private/sources") && !text.contains("private-folder")));
    // Rehydration still starts from the full Host state, not a chain of model-only patches.
    let stored = list_active_journal_entries(&connection, "conversation").unwrap();
    assert_eq!(stored[1].record.revision(), current.revision);
    assert!(serde_json::to_string(&stored[1].record)
        .unwrap()
        .contains("/private/sources/site"));
}

#[test]
fn workspace_binding_edit_resend_recomputes_patch_from_the_retained_source_map() {
    let mut connection = connection();
    insert_conversation(&connection, "conversation");
    insert_message(&connection, "conversation", "edited-input", 1);
    insert_message(&connection, "conversation", "later-input", 2);
    let initial = workspace_snapshot(0, ("docs", "docs-old"));
    let abandoned = workspace_snapshot(1, ("website", "site"));
    append(
        &mut connection,
        "conversation",
        1,
        None,
        &WorldStateRecord::Full(initial.clone()),
        10,
    )
    .unwrap();
    append(
        &mut connection,
        "conversation",
        1,
        Some("later-input"),
        &WorldStateRecord::Diff(WorldStateDiff::between(&initial, &abandoned).unwrap()),
        11,
    )
    .unwrap();

    // Editing an earlier message discards the future state even without a direct anchor on it.
    chat_repository::delete_messages(
        &mut connection,
        "conversation",
        &["edited-input".into(), "later-input".into()],
    )
    .unwrap();
    let retained = fold_active_snapshot(&connection, "conversation")
        .unwrap()
        .unwrap();
    assert_eq!(retained, initial);
    insert_message(&connection, "conversation", "resent-input", 1);
    let replacement = workspace_snapshot(1, ("docs", "docs-new"));
    append(
        &mut connection,
        "conversation",
        1,
        Some("resent-input"),
        &WorldStateRecord::Diff(WorldStateDiff::between(&retained, &replacement).unwrap()),
        20,
    )
    .unwrap();
    let rendered = projected_journal(&connection);
    assert_eq!(rendered.len(), 2);
    assert!(rendered[1].contains("\"op\":\"patch\""));
    assert!(rendered[1].contains("source_replaced"));
    assert!(!rendered[1].contains("website"));
    assert!(!rendered[1].contains("\"kind\":\"added\""));
    assert!(!rendered[1].contains("\"kind\":\"removed\""));
    assert_eq!(
        fold_active_snapshot(&connection, "conversation").unwrap(),
        Some(replacement)
    );
}
