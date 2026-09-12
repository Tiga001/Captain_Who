fn fork_workspace_snapshot(sequence: u64, auxiliary: &str) -> WorldStateSnapshot {
    let workspace = crate::AgentWorkspaceContext {
        project_id: Some("private-project".into()),
        display_name: Some("project".into()),
        root_path: Some("/private/sources/app".into()),
        folders: ["app", auxiliary]
            .into_iter()
            .map(|alias| crate::workspace::WorkspaceFolder {
                id: format!("private-folder-{alias}"),
                alias: alias.into(),
                role: if alias == "app" {
                    crate::storage::models::ProjectFolderRole::Primary
                } else {
                    crate::storage::models::ProjectFolderRole::Auxiliary
                },
                path: format!("/private/sources/{alias}"),
                canonical_path: Some(format!("/private/sources/{alias}")),
                directory_identity: Some(crate::file_change::FileChangeDirectoryIdentity::Unix {
                    schema_version: 1,
                    device: 1,
                    inode: 1,
                }),
            })
            .collect(),
    };
    WorldStateSnapshot::new(
        "workspace-source",
        sequence,
        vec![crate::world_state::workspace_binding_section(
            Some(&workspace),
            WorldStateLifetime::Conversation,
        )
        .unwrap()],
    )
    .unwrap()
}

#[test]
fn workspace_binding_fork_preserves_only_cutoff_source_patches_and_uses_that_base_next() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let initial = fork_workspace_snapshot(0, "docs");
    let at_cutoff = fork_workspace_snapshot(1, "website");
    let future = fork_workspace_snapshot(2, "future-only");
    for (record, anchor, created_at) in [
        (WorldStateRecord::Full(initial.clone()), None, 10),
        (
            WorldStateRecord::Diff(WorldStateDiff::between(&initial, &at_cutoff).unwrap()),
            Some("user-b"),
            30,
        ),
        (
            WorldStateRecord::Diff(WorldStateDiff::between(&at_cutoff, &future).unwrap()),
            Some("user-c"),
            50,
        ),
    ] {
        world_state_repository::append_record(
            &mut connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &source.id,
                epoch_generation: 1,
                base_summary_id: None,
                effective_before_message_id: anchor,
                request_boundary: None,
                model_observed: true,
                record: &record,
                created_at,
            },
        )
        .unwrap();
    }
    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-workspace",
        &source.id,
        "assistant-b",
        100,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &plan).unwrap();
    let target =
        world_state_repository::list_active_journal_entries(&connection, &plan.target.id).unwrap();
    assert_eq!(target.len(), 2);
    assert_eq!(
        target[1].effective_before_message_id.as_deref(),
        Some(plan.message_id_map["user-b"].as_str())
    );
    let WorldStateRecord::Full(baseline) = &target[0].record else {
        panic!("missing full baseline")
    };
    let WorldStateRecord::Diff(diff) = &target[1].record else {
        panic!("missing cutoff diff")
    };
    let projected = diff
        .model_projection_against(baseline, WorldStateLifetime::Conversation)
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    assert!(projected.contains("\"op\":\"patch\""));
    assert!(projected.contains("@workspace/website"));
    assert!(!projected.contains("future-only"));
    let fork_head = world_state_repository::fold_active_snapshot(&connection, &plan.target.id)
        .unwrap()
        .unwrap();
    assert_eq!(fork_head.sections, at_cutoff.sections);
    // The next run compares with the state visible at the fork, never the source's future head.
    let next = WorldStateSnapshot::new(
        fork_head.epoch_id.clone(),
        fork_head.sequence + 1,
        initial.sections,
    )
    .unwrap();
    let next_patch = WorldStateDiff::between(&fork_head, &next)
        .unwrap()
        .model_projection_against(&fork_head, WorldStateLifetime::Conversation)
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    assert!(next_patch.contains("\"alias\":\"website\",\"kind\":\"removed\""));
    assert!(next_patch.contains("@workspace/docs"));
    assert!(!next_patch.contains("future-only"));
    assert!(!next_patch.contains("/private/sources"));
}
