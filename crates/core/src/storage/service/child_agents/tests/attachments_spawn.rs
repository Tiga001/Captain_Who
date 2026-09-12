use super::*;

#[test]
fn host_child_spawn_freezes_workspace_before_dispatch_and_preserves_replay() {
    let fixture = Fixture::new(Some("model-a"));
    let first_root = fixture._directory.path().join("first");
    let next_root = fixture._directory.path().join("next");
    std::fs::create_dir(&first_root).unwrap();
    std::fs::create_dir(&next_root).unwrap();
    let mut project = ProjectRecord::with_primary_folder(
        "project-a",
        "Project A",
        first_root.to_string_lossy(),
        1,
    );
    fixture.service.save_project(project.clone()).unwrap();
    let input = spawn_input("host-workspace-spawn", "workspace-child");
    let first = fixture.service.create_child_agent(&input).unwrap();
    let frozen = fixture
        .service
        .load_agent_workspace_for_wake(&first.initial_wake.wake_id)
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(frozen.root_path.as_deref(), first_root.to_str());

    project.folders[0].path = next_root.to_string_lossy().into_owned();
    fixture.service.save_project(project).unwrap();
    let replay = fixture.service.create_child_agent(&input).unwrap();
    assert_eq!(replay.initial_wake.wake_id, first.initial_wake.wake_id);
    assert_eq!(
        fixture
            .service
            .load_agent_workspace_for_wake(&replay.initial_wake.wake_id)
            .unwrap(),
        Some(Some(frozen))
    );
    let next = fixture
        .service
        .create_child_agent(&spawn_input("next-host-spawn", "next-child"))
        .unwrap();
    let next_workspace = fixture
        .service
        .load_agent_workspace_for_wake(&next.initial_wake.wake_id)
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(next_workspace.root_path.as_deref(), next_root.to_str());
}

#[test]
fn all_snapshot_copies_attachment_to_independent_path_and_retry_is_idempotent() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let source_attachment = attach_file_to_first_user_message(&fixture, "all", true);
    let mut input = spawn_input("spawn-all-attachment", "all_attachment");
    input.fork_turns = AgentForkTurns::All;
    let created = fixture.service.create_child_agent(&input).unwrap();

    let connection = fixture.service.state.connection().unwrap();
    let target_attachment = attachment_repository::list_conversation_attachments(
        &connection,
        &created.agent.conversation_id,
    )
    .unwrap()
    .into_iter()
    .next()
    .unwrap();
    assert_ne!(target_attachment.id, source_attachment.id);
    assert_ne!(
        target_attachment.storage_rel_path,
        source_attachment.storage_rel_path
    );
    assert_eq!(
        fs::read(
            fixture
                .service
                .attachment_root
                .join(&target_attachment.storage_rel_path)
        )
        .unwrap(),
        fs::read(
            fixture
                .service
                .attachment_root
                .join(&source_attachment.storage_rel_path)
        )
        .unwrap()
    );
    drop(connection);

    assert_eq!(fixture.service.create_child_agent(&input).unwrap(), created);
    let connection = fixture.service.state.connection().unwrap();
    assert_eq!(
        attachment_repository::list_conversation_attachments(
            &connection,
            &created.agent.conversation_id,
        )
        .unwrap()
        .len(),
        1
    );
}

#[test]
fn missing_snapshot_attachment_rolls_back_all_database_facts_without_files() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    attach_file_to_first_user_message(&fixture, "missing", false);
    let mut input = spawn_input("spawn-missing-attachment", "missing_attachment");
    input.fork_turns = AgentForkTurns::All;
    assert!(matches!(
        fixture.service.create_child_agent(&input),
        Err(ChildAgentSpawnError::SnapshotUnavailable(_))
    ));
    let connection = fixture.service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id IS NOT NULL",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id != 'root-conversation'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    drop(connection);
    assert!(regular_files_below(&fixture.service.attachment_root).is_empty());
}

#[test]
fn database_failure_after_attachment_publication_removes_copied_file() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let source = attach_file_to_first_user_message(&fixture, "rollback-file", true);
    {
        let connection = fixture.service.state.connection().unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TRIGGER fail_child_wake_after_attachment
                     BEFORE INSERT ON agent_wake_requests
                     BEGIN
                         SELECT RAISE(ABORT, 'injected wake failure after attachment');
                     END;",
            )
            .unwrap();
    }
    let mut input = spawn_input("spawn-file-rollback", "file_rollback");
    input.fork_turns = AgentForkTurns::All;
    assert!(matches!(
        fixture.service.create_child_agent(&input),
        Err(ChildAgentSpawnError::StorageUnavailable(_))
    ));
    assert_eq!(
        regular_files_below(&fixture.service.attachment_root),
        vec![fixture
            .service
            .attachment_root
            .join(source.storage_rel_path)]
    );
    let connection = fixture.service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id IS NOT NULL",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM child_context_snapshots", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn last_zero_is_rejected_before_any_child_fact_is_written() {
    let fixture = Fixture::new(Some("model-a"));
    let mut input = spawn_input("snapshot-zero", "zero");
    input.fork_turns = AgentForkTurns::Last(0);
    assert!(matches!(
        fixture.service.create_child_agent(&input),
        Err(ChildAgentSpawnError::InvalidInput {
            field: "fork_turns",
            ..
        })
    ));
    assert!(fixture
        .service
        .list_agent_children("agent-root", "agent-root")
        .unwrap()
        .is_empty());
}

#[test]
fn unexecutable_identity_names_are_rejected_before_spawn_but_payloads_remain_multiline() {
    let fixture = Fixture::new(Some("model-a"));
    let mut invalid = spawn_input("invalid-identity", "bad\nname");
    invalid.task = "line one\nline two".to_string();
    assert!(matches!(
        fixture.service.create_child_agent(&invalid),
        Err(ChildAgentSpawnError::InvalidInput {
            field: "task_name",
            ..
        })
    ));
    assert!(fixture
        .service
        .list_agent_children("agent-root", "agent-root")
        .unwrap()
        .is_empty());

    let mut multiline = spawn_input("multiline-task", "multiline_task");
    multiline.task = "line one\nline two".to_string();
    assert_eq!(
        fixture
            .service
            .create_child_agent(&multiline)
            .unwrap()
            .collaboration_identity
            .entrusted_task,
        multiline.task
    );
}

#[test]
fn reserved_root_name_is_rejected_at_every_child_depth_without_any_spawn_facts() {
    let fixture = Fixture::new(Some("model-a"));
    let first = fixture
        .service
        .create_child_agent(&spawn_input("ordinary-child", "ordinary_child"))
        .unwrap();
    let mut nested_input = spawn_input("ordinary-grandchild", "ordinary_grandchild");
    nested_input.parent_agent_id = first.agent.agent_id.clone();
    let second = fixture.service.create_child_agent(&nested_input).unwrap();
    let counts = || {
        let connection = fixture.service.state.connection().unwrap();
        [
            "agent_nodes",
            "conversations",
            "messages",
            "agent_mailbox_messages",
            "agent_wake_requests",
            "child_context_snapshots",
        ]
        .map(|table| {
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap()
        })
    };
    let before = counts();
    // Root uses the fixture name Root, so a reserved-name rejection cannot be a duplicate-name collision.
    for (index, parent) in [
        "agent-root",
        first.agent.agent_id.as_str(),
        second.agent.agent_id.as_str(),
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = spawn_input(
            &format!("reserved-child-{index}"),
            crate::ROOT_AGENT_TASK_NAME,
        );
        input.parent_agent_id = parent.to_string();
        for _ in 0..2 {
            assert_eq!(
                fixture.service.create_child_agent(&input),
                Err(ChildAgentSpawnError::InvalidInput {
                    field: "task_name",
                    reason: crate::ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE.to_string(),
                })
            );
            assert_eq!(counts(), before);
        }
    }
    // An existing idempotency key must not turn a reserved-name attempt into a replay or a write.
    let mut reused = spawn_input("ordinary-child", crate::ROOT_AGENT_TASK_NAME);
    assert_eq!(
        fixture.service.create_child_agent(&reused),
        Err(ChildAgentSpawnError::InvalidInput {
            field: "task_name",
            reason: crate::ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE.to_string(),
        })
    );
    assert_eq!(counts(), before);
    reused.task_name = "ordinary_child".to_string();
    reused.task = "Perform ordinary_child and report evidence.".to_string();
    assert_eq!(
        fixture.service.create_child_agent(&reused).unwrap().agent,
        first.agent
    );
    assert_eq!(counts(), before);
}

#[test]
fn atomic_spawn_persists_independent_conversation_task_projection_and_queued_wake() {
    let fixture = Fixture::new(Some("model-a"));
    let input = spawn_input("spawn-1", "security_review");
    let created = fixture.service.create_child_agent(&input).unwrap();

    assert_eq!(created.agent.parent_agent_id.as_deref(), Some("agent-root"));
    assert_ne!(created.agent.conversation_id, "root-conversation");
    assert_eq!(
        created.model_selection_source,
        AgentModelSelectionSource::Parent
    );
    assert_eq!(
        created
            .agent
            .model_snapshot
            .as_ref()
            .unwrap()
            .model_config_id,
        "model-a"
    );
    assert_eq!(
        created.task_message.delivery_status,
        AgentMailboxDeliveryStatus::Acknowledged
    );
    assert_eq!(created.initial_wake.status, AgentWakeStatus::Queued);
    assert_eq!(
        fixture
            .service
            .conversation_message_origin(
                &created.agent.conversation_id,
                &created.task_message.projection_message_id,
            )
            .unwrap(),
        crate::ConversationMessageOrigin::Agent {
            sender_agent_id: "agent-root".to_string(),
            source_agent_message_id: created.task_message.message_id.clone(),
        }
    );
    let trusted = fixture
        .service
        .resolve_child_agent_wake(
            &created.agent.agent_id,
            &created.initial_wake.wake_id,
            &created.task_message.message_id,
        )
        .unwrap();
    assert_eq!(trusted.collaboration_identity.entrusted_task, input.task);

    let retry = fixture.service.create_child_agent(&input).unwrap();
    assert_eq!(retry, created);
    let claim_token = "controlled-turn-claim";
    let claimed = fixture
        .service
        .claim_next_agent_wake(&created.agent.agent_id, claim_token)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, created.initial_wake.wake_id);
    fixture
        .service
        .transition_agent_wake(
            &claimed.wake_id,
            AgentWakeStatus::Claimed,
            AgentWakeStatus::Running,
            Some(claim_token),
        )
        .unwrap();
    assert!(matches!(
        fixture.service.resolve_running_child_agent_wake(
            &created.agent.agent_id,
            &created.initial_wake.wake_id,
            &created.task_message.message_id,
            "wrong-claim-token",
        ),
        Err(AgentGraphError::Conflict(_))
    ));
    let authorized = fixture
        .service
        .resolve_running_child_agent_wake(
            &created.agent.agent_id,
            &created.initial_wake.wake_id,
            &created.task_message.message_id,
            claim_token,
        )
        .unwrap();
    assert_eq!(authorized.initial_wake.status, AgentWakeStatus::Running);
    let recovered = fixture
        .service
        .resolve_active_child_agent_wake_by_identity(&authorized.collaboration_identity)
        .unwrap();
    assert_eq!(recovered.claim_token, claim_token);
    assert_eq!(recovered.spawn, authorized);
    fixture
        .service
        .transition_agent_wake(
            &created.initial_wake.wake_id,
            AgentWakeStatus::Running,
            AgentWakeStatus::WaitingForApproval,
            Some(claim_token),
        )
        .unwrap();
    assert_eq!(
        fixture
            .service
            .resolve_active_child_agent_wake_by_identity(&authorized.collaboration_identity)
            .unwrap()
            .spawn
            .initial_wake
            .status,
        AgentWakeStatus::WaitingForApproval
    );
    let mut forged_identity = authorized.collaboration_identity;
    forged_identity.entrusted_task.push_str(" forged");
    assert!(matches!(
        fixture
            .service
            .resolve_active_child_agent_wake_by_identity(&forged_identity),
        Err(AgentGraphError::Conflict(_))
    ));
    let connection = fixture.service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id = 'agent-root'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1",
                [&created.agent.conversation_id],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn request_reuse_with_changed_task_or_selector_is_rejected() {
    let fixture = Fixture::new(Some("model-a"));
    let input = spawn_input("spawn-conflict", "review");
    fixture.service.create_child_agent(&input).unwrap();

    let mut changed_task = input.clone();
    changed_task.task.push_str(" changed");
    assert!(matches!(
        fixture.service.create_child_agent(&changed_task),
        Err(ChildAgentSpawnError::IdempotencyConflict(_))
    ));
    let mut changed_selector = input.clone();
    changed_selector.explicit_model_id = Some("model-c".to_string());
    assert!(matches!(
        fixture.service.create_child_agent(&changed_selector),
        Err(ChildAgentSpawnError::IdempotencyConflict(_))
    ));
    let mut changed_reasoning = input;
    changed_reasoning.reasoning_effort = Some(ReasoningEffort::High);
    assert!(matches!(
        fixture.service.create_child_agent(&changed_reasoning),
        Err(ChildAgentSpawnError::IdempotencyConflict(_))
    ));
}

#[test]
fn creation_request_reuse_under_another_parent_is_typed_idempotency_conflict() {
    let fixture = Fixture::new(Some("model-a"));
    let first = fixture
        .service
        .create_child_agent(&spawn_input("shared-request", "first_child"))
        .unwrap();
    let sibling = fixture
        .service
        .create_child_agent(&spawn_input("sibling-request", "second_child"))
        .unwrap();
    let mut reused = spawn_input("shared-request", "grandchild");
    reused.parent_agent_id = sibling.agent.agent_id.clone();
    assert!(matches!(
        fixture.service.create_child_agent(&reused),
        Err(ChildAgentSpawnError::IdempotencyConflict(_))
    ));
    let tree = fixture.service.list_agent_tree("agent-root").unwrap();
    assert_eq!(tree.len(), 3);
    assert_eq!(
        tree.iter()
            .filter(|node| node.parent_agent_id.as_deref() == Some(&sibling.agent.agent_id))
            .count(),
        0
    );
    assert_eq!(
        first.agent.creation_request_id,
        "shared-request".to_string()
    );
}
