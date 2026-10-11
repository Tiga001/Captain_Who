use super::*;
use rusqlite::types::Value as SqlValue;
use std::collections::BTreeMap;

/// Observe all application tables from a separate read-only disk connection. A refused
/// membership check must not leave a journal, FTS, draft, Agent or other deletion side effect.
fn disk_facts(path: &std::path::Path) -> BTreeMap<String, Vec<Vec<SqlValue>>> {
    let mut connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let snapshot = connection.transaction().unwrap();
    let tables = {
        let mut query = snapshot
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        let tables = query
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        tables
    };
    let mut facts = BTreeMap::new();
    for table in tables {
        let quoted = table.replace('"', "\"\"");
        let mut query = snapshot
            .prepare(&format!("SELECT * FROM \"{quoted}\""))
            .unwrap();
        let columns = query.column_count();
        let mut rows = query
            .query_map([], |row| {
                (0..columns)
                    .map(|column| row.get::<_, SqlValue>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows.sort_by_cached_key(|row| format!("{row:?}"));
        facts.insert(table, rows);
    }
    snapshot.commit().unwrap();
    facts
}

#[derive(Clone, Copy, Debug)]
enum MembershipChange {
    Added,
    MovedIn,
    MovedOut,
    Removed,
    IncompleteSnapshot,
}

fn assert_checked_delete(change: MembershipChange, graph_bound: bool) {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for (id, project) in [
        ("stable", "project-1"),
        (
            "changing",
            if matches!(change, MembershipChange::MovedIn) {
                "project-2"
            } else {
                "project-1"
            },
        ),
        ("unrelated", "project-2"),
    ] {
        service
            .save_conversation(conversation(id, Some(project), &format!("message-{id}")))
            .unwrap();
    }
    if graph_bound {
        bind_agent_root(&service, "stable-root", "stable");
    }
    service
        .save_composer_draft(composer_draft(
            "stable",
            Some("project-1"),
            "Preserve until checked delete commits",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "unrelated",
            Some("project-2"),
            "Never delete this draft",
        ))
        .unwrap();
    let mut drained = vec!["stable".to_string()];
    if !matches!(
        change,
        MembershipChange::MovedIn | MembershipChange::IncompleteSnapshot
    ) {
        drained.push("changing".into());
    }

    match change {
        MembershipChange::Added => {
            service
                .save_conversation(conversation("late", Some("project-1"), "message-late"))
                .unwrap();
        }
        MembershipChange::MovedIn | MembershipChange::MovedOut => {
            let mut moved = service.load_conversation("changing").unwrap().unwrap();
            moved.project_id = Some(
                if matches!(change, MembershipChange::MovedIn) {
                    "project-1"
                } else {
                    "project-2"
                }
                .into(),
            );
            service.save_conversation(moved).unwrap();
        }
        MembershipChange::Removed => service.delete_conversation("changing").unwrap(),
        MembershipChange::IncompleteSnapshot => {}
    }

    let path = fixture.root.join("storage.sqlite");
    let before = disk_facts(&path);
    let error = service
        .delete_project_checked_members("project-1", &drained)
        .unwrap_err();
    assert!(
        error.contains("membership changed"),
        "{change:?}, graph={graph_bound}: {error}"
    );
    assert_eq!(
        disk_facts(&path),
        before,
        "refused {change:?} deletion must have no durable side effects, graph={graph_bound}"
    );
    assert!(service
        .load_projects()
        .unwrap()
        .iter()
        .any(|project| project.id == "project-1"));
    assert!(service.load_conversation("stable").unwrap().is_some());
    assert!(service.load_conversation("unrelated").unwrap().is_some());
    if !matches!(change, MembershipChange::Removed) {
        assert!(service.load_conversation("changing").unwrap().is_some());
    }

    let mut current = service
        .load_conversations()
        .unwrap()
        .into_iter()
        .filter(|conversation| conversation.project_id.as_deref() == Some("project-1"))
        .map(|conversation| conversation.id)
        .collect::<Vec<_>>();
    // The contract is an exact set, independent of snapshot order or duplicate hints.
    current.reverse();
    current.push("stable".into());
    service
        .delete_project_checked_members("project-1", &current)
        .unwrap();
    assert!(!service
        .load_projects()
        .unwrap()
        .iter()
        .any(|project| project.id == "project-1"));
    for conversation in &current {
        assert!(service.load_conversation(conversation).unwrap().is_none());
    }
    assert!(service.load_conversation("unrelated").unwrap().is_some());
    if matches!(change, MembershipChange::MovedOut) {
        assert_eq!(
            service
                .load_conversation("changing")
                .unwrap()
                .unwrap()
                .project_id
                .as_deref(),
            Some("project-2")
        );
    }
    assert_eq!(
        service
            .load_composer_drafts()
            .unwrap()
            .iter()
            .map(|draft| draft.scope_id.as_str())
            .collect::<Vec<_>>(),
        vec!["unrelated"]
    );
    if graph_bound {
        assert!(service
            .get_agent_node_by_conversation("stable")
            .unwrap()
            .is_none());
    }
    drop(service);
    let reopened =
        StorageService::open_with_model_credentials(&path, fixture.model_credentials.clone())
            .unwrap();
    assert!(!reopened
        .load_projects()
        .unwrap()
        .iter()
        .any(|project| project.id == "project-1"));
    assert!(reopened.load_conversation("unrelated").unwrap().is_some());
}

#[test]
fn checked_project_membership_rejects_new_member_without_deleting_any_facts() {
    for graph in [false, true] {
        assert_checked_delete(MembershipChange::Added, graph);
    }
}

#[test]
fn checked_project_membership_rejects_moved_in_member_without_deleting_any_facts() {
    for graph in [false, true] {
        assert_checked_delete(MembershipChange::MovedIn, graph);
    }
}

#[test]
fn checked_project_membership_rejects_moved_out_member_without_deleting_any_facts() {
    for graph in [false, true] {
        assert_checked_delete(MembershipChange::MovedOut, graph);
    }
}

#[test]
fn checked_project_membership_rejects_removed_member_without_deleting_any_facts() {
    for graph in [false, true] {
        assert_checked_delete(MembershipChange::Removed, graph);
    }
}

#[test]
fn checked_project_membership_rejects_incomplete_snapshot_without_deleting_any_facts() {
    for graph in [false, true] {
        assert_checked_delete(MembershipChange::IncompleteSnapshot, graph);
    }
}
