use super::*;
use crate::storage::service::conversations::fork_staging::set_prepare_hook;

fn external_connection(path: &Path) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .busy_timeout(std::time::Duration::from_millis(500))
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection
}

#[test]
fn child_snapshot_copy_releases_writer_and_rejects_changed_source() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let source = attach_file_to_first_user_message(&fixture, "prepare-race", true);
    let database = fixture._directory.path().join("storage.sqlite");
    set_prepare_hook(move || {
        // This would time out under the former IMMEDIATE transaction across file copying.
        external_connection(&database)
            .execute(
                "UPDATE messages SET content='edited during staging' WHERE id='root-user-0'",
                [],
            )
            .unwrap();
    });
    let mut input = spawn_input("changed-source", "changed-source");
    input.fork_turns = AgentForkTurns::All;
    assert!(
        matches!(fixture.service.create_child_agent(&input), Err(ChildAgentSpawnError::Conflict(message))
        if message.contains("parent history changed"))
    );
    assert_eq!(
        regular_files_below(&fixture.service.attachment_root),
        vec![fixture
            .service
            .attachment_root
            .join(source.storage_rel_path)]
    );
    assert_eq!(
        fixture.service.list_agent_tree("agent-root").unwrap().len(),
        1
    );
}

#[test]
fn no_history_child_does_not_conflict_with_unrelated_parent_history_progress() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let database = fixture._directory.path().join("storage.sqlite");
    set_prepare_hook(move || {
        external_connection(&database)
            .execute(
                "UPDATE messages SET content='edited during staging' WHERE id='root-user-0'",
                [],
            )
            .unwrap();
    });
    let child = fixture
        .service
        .create_child_agent(&spawn_input("none-race", "none-race"))
        .unwrap();
    assert_eq!(child.agent.parent_agent_id.as_deref(), Some("agent-root"));
    assert_eq!(
        fixture.service.list_agent_tree("agent-root").unwrap().len(),
        2
    );
}

#[test]
fn tree_stop_during_child_prepare_blocks_final_admission() {
    let fixture = Fixture::new(Some("model-a"));
    insert_active_root_trace(&fixture, "preparing-root-run", "preparing-root-assistant");
    let database = fixture._directory.path().join("storage.sqlite");
    set_prepare_hook(move || {
        let mut connection = external_connection(&database);
        let stopped = agent_graph_repository::begin_agent_tree_run_stop_by_root_agent(
            &mut connection,
            "agent-root",
            "preparing-root-run",
            21,
        )
        .unwrap();
        assert!(stopped.is_some());
    });
    let result = fixture
        .service
        .create_child_agent_with_limits_and_expected_selector_from_run(
            &spawn_input("stopped-before-commit", "stopped-before-commit"),
            AgentTreeResourceLimits::default(),
            None,
            None,
            "preparing-root-run",
        );
    assert!(result.is_err());
    assert_eq!(
        fixture.service.list_agent_tree("agent-root").unwrap().len(),
        1
    );
    assert_eq!(
        fixture
            .service
            .state
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM agent_wake_requests", [], |row| row
                .get::<_, u64>(0),)
            .unwrap(),
        0
    );
}

#[test]
fn child_spawn_preparation_race_returns_same_receipt_and_cleans_only_loser_files() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    attach_file_to_first_user_message(&fixture, "same-request", true);
    let database = fixture._directory.path().join("storage.sqlite");
    let credentials = fixture.service.model_credentials.clone();
    let mut input = spawn_input("same-request", "same-request");
    input.fork_turns = AgentForkTurns::All;
    let competing_input = input.clone();
    let winner = Arc::new(std::sync::Mutex::new(None));
    let published_winner = winner.clone();
    set_prepare_hook(move || {
        let other = StorageService::open_with_model_credentials(&database, credentials).unwrap();
        let record = other.create_child_agent(&competing_input).unwrap();
        *published_winner.lock().unwrap() = Some(record.agent.conversation_id);
    });
    let created = fixture.service.create_child_agent(&input).unwrap();
    assert_eq!(Some(created.agent.conversation_id), *winner.lock().unwrap());
    assert_eq!(
        fixture.service.list_agent_tree("agent-root").unwrap().len(),
        2
    );
    assert_eq!(
        regular_files_below(&fixture.service.attachment_root).len(),
        2
    );
}

#[test]
fn child_snapshot_rejects_diff_payload_changed_in_the_same_millisecond() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let identity = crate::AgentTurnDiffIdentity {
        run_id: "root-run-0".into(),
        conversation_id: "root-conversation".into(),
        assistant_message_id: "root-assistant-0".into(),
        project_id: "project-a".into(),
        workspace_root: fixture._directory.path().to_string_lossy().into_owned(),
    };
    let change = crate::AgentTurnFileChange {
        path: "same-clock.txt".into(),
        before: crate::AgentTurnFileContent::Missing,
        after: crate::AgentTurnFileContent::Text("first version".into()),
    };
    {
        let mut connection = fixture.service.state.connection().unwrap();
        crate::storage::turn_diff_repository::record_file_change(
            &mut connection,
            &identity,
            "first-change",
            &change,
            100,
        )
        .unwrap();
    }
    let database = fixture._directory.path().join("storage.sqlite");
    set_prepare_hook(move || {
        let mut connection = external_connection(&database);
        crate::storage::turn_diff_repository::record_file_change(
            &mut connection,
            &identity,
            "same-millisecond-change",
            &crate::AgentTurnFileChange {
                after: crate::AgentTurnFileContent::Text("second version".into()),
                ..change
            },
            100,
        )
        .unwrap();
        let timestamp: i64 = connection.query_row(
            "SELECT updated_at FROM agent_turn_diffs WHERE assistant_message_id='root-assistant-0'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(
            timestamp, 100,
            "timestamp-only fencing would miss this change"
        );
    });
    let mut input = spawn_input("same-clock-diff", "same-clock-diff");
    input.fork_turns = AgentForkTurns::All;
    assert!(matches!(fixture.service.create_child_agent(&input),
        Err(ChildAgentSpawnError::Conflict(message)) if message.contains("side records changed")));
    assert_eq!(
        fixture.service.list_agent_tree("agent-root").unwrap().len(),
        1
    );
}

#[test]
fn child_snapshot_does_not_recreate_an_artifact_grant_removed_during_staging() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let sha256 = "a".repeat(64);
    let artifact_id = format!("sha256:{sha256}");
    {
        let mut connection = fixture.service.state.connection().unwrap();
        crate::storage::managed_artifact_repository::register(
            &mut connection,
            &crate::storage::managed_artifact_repository::ManagedArtifactRecord {
                artifact_id: artifact_id.clone(),
                kind: crate::storage::managed_artifact_repository::ManagedArtifactKind::Document,
                storage_relative_path: "documents/snapshot-fixture.pdf".into(),
                format: "pdf".into(),
                media_type: "application/pdf".into(),
                size_bytes: 10,
                sha256,
                width: None,
                height: None,
                created_at: 1,
            },
        )
        .unwrap();
        crate::storage::managed_artifact_repository::grant(
            &mut connection,
            &crate::storage::managed_artifact_repository::ManagedArtifactGrant {
                artifact_id: artifact_id.clone(),
                conversation_id: "root-conversation".into(),
                run_id: "root-run-0".into(),
                call_id: "artifact-call".into(),
                created_at: 1,
            },
        )
        .unwrap();
    }
    let database = fixture._directory.path().join("storage.sqlite");
    set_prepare_hook(move || {
        let removed = external_connection(&database).execute(
            "DELETE FROM managed_artifact_grants WHERE artifact_id=?1 AND conversation_id='root-conversation'",
            [&artifact_id],
        ).unwrap();
        assert_eq!(removed, 1);
    });
    let mut input = spawn_input("removed-artifact-grant", "removed-artifact-grant");
    input.fork_turns = AgentForkTurns::All;
    assert!(matches!(fixture.service.create_child_agent(&input),
        Err(ChildAgentSpawnError::Conflict(message)) if message.contains("side records changed")));
    let connection = fixture.service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM managed_artifact_grants", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    drop(connection);
    assert_eq!(
        fixture.service.list_agent_tree("agent-root").unwrap().len(),
        1
    );
}
