use super::*;

#[test]
fn root_reserved_task_name_is_valid_for_a_root() {
    let mut connection = connection();
    insert_conversation(&connection, "conversation-root-name", None);
    let input = EnsureRootAgentInput {
        agent_id: "agent-root-name".to_string(),
        conversation_id: "conversation-root-name".to_string(),
        creation_request_id: "ensure-root-name".to_string(),
        task_name: crate::ROOT_AGENT_TASK_NAME.to_string(),
    };
    let root = ensure_root_agent(&mut connection, &input, 10).unwrap();
    assert_eq!(root.record().task_name, "主智能体");
    assert!(matches!(
        ensure_root_agent(&mut connection, &input, 11).unwrap(),
        IdempotentCreate::Existing(_)
    ));
}

#[test]
fn root_reserved_task_name_is_rejected_for_children_at_every_depth_without_writes() {
    // The fixture root has a different name: this must not depend on UNIQUE collisions.
    let mut connection = setup_tree();
    let original_tree = list_agent_tree(&connection, "agent-root").unwrap();
    for (parent, path) in [
        ("agent-root", "/root/主智能体"),
        ("agent-child", "/root/review/主智能体"),
    ] {
        let input = child_input(
            "reserved-child",
            "agent-root",
            parent,
            "conversation-grand",
            crate::ROOT_AGENT_TASK_NAME,
            path,
        );
        for _ in 0..2 {
            assert_eq!(
                create_agent_node(&mut connection, &input, 20),
                Err(AgentGraphError::InvalidInput {
                    field: "task_name",
                    reason: crate::ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE.to_string(),
                })
            );
            assert_eq!(
                list_agent_tree(&connection, "agent-root").unwrap(),
                original_tree
            );
        }
    }

    // The fork insertion boundary also rejects a child record before writing anything.
    let mut forked = original_tree
        .iter()
        .find(|node| node.parent_agent_id.is_some())
        .unwrap()
        .clone();
    forked.agent_id = "reserved-fork-child".to_string();
    forked.conversation_id = "conversation-grand".to_string();
    forked.creation_request_id = "reserved-fork-child-request".to_string();
    forked.task_name = crate::ROOT_AGENT_TASK_NAME.to_string();
    forked.task_path = "/root/主智能体".to_string();
    assert_eq!(
        insert_forked_agent_node_in_transaction(&connection, &forked),
        Err(AgentGraphError::InvalidInput {
            field: "task_name",
            reason: crate::ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE.to_string(),
        })
    );
    assert_eq!(
        list_agent_tree(&connection, "agent-root").unwrap(),
        original_tree
    );
}

#[test]
fn root_and_multilevel_tree_are_idempotent_and_tree_scoped() {
    let mut connection = setup_tree();
    let root_retry = ensure_root_agent(
        &mut connection,
        &EnsureRootAgentInput {
            agent_id: "agent-root".to_string(),
            conversation_id: "conversation-root".to_string(),
            creation_request_id: "ensure-agent-root".to_string(),
            task_name: "Root".to_string(),
        },
        99,
    )
    .unwrap();
    assert!(matches!(root_retry, IdempotentCreate::Existing(_)));

    let grandchild = create_agent_node(
        &mut connection,
        &child_input(
            "agent-grand",
            "agent-root",
            "agent-child",
            "conversation-grand",
            "evidence",
            "/root/review/evidence",
        ),
        12,
    )
    .unwrap();
    assert!(matches!(grandchild, IdempotentCreate::Created(_)));
    assert_eq!(list_agent_tree(&connection, "agent-root").unwrap().len(), 3);
    assert_eq!(
        list_agent_children(&connection, "agent-root", "agent-child")
            .unwrap()
            .len(),
        1
    );

    insert_project(&connection, "project-b");
    insert_conversation(&connection, "conversation-root-b", Some("project-b"));
    insert_conversation(&connection, "conversation-cross", Some("project-b"));
    ensure_root(&mut connection, "agent-root-b", "conversation-root-b");
    let cross_tree = create_agent_node(
        &mut connection,
        &child_input(
            "agent-cross",
            "agent-root-b",
            "agent-child",
            "conversation-cross",
            "cross",
            "/root/review/cross",
        ),
        13,
    );
    assert!(matches!(cross_tree, Err(AgentGraphError::Conflict(_))));

    insert_conversation(&connection, "conversation-duplicate", Some("project-a"));
    let duplicate_name = create_agent_node(
        &mut connection,
        &child_input(
            "agent-duplicate",
            "agent-root",
            "agent-root",
            "conversation-duplicate",
            "review",
            "/root/review",
        ),
        14,
    );
    assert!(matches!(duplicate_name, Err(AgentGraphError::Conflict(_))));
}

#[test]
fn effective_permissions_inherit_direct_parent_and_meet_every_ancestor_snapshot() {
    let mut connection = setup_tree();
    create_agent_node(
        &mut connection,
        &child_input(
            "agent-grand",
            "agent-root",
            "agent-child",
            "conversation-grand",
            "grand",
            "/root/review/grand",
        ),
        12,
    )
    .unwrap();
    let custom_parent = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        patch: AgentPatchPermission::AutoApprove,
        builtin_execution: AgentBuiltinExecutionPermission::RequireApproval,
    };
    record_permissions_for_test_turn(
        &mut connection,
        "agent-root",
        "conversation-root",
        "root-full",
        full_permissions(),
        20,
    );
    record_permissions_for_test_turn(
        &mut connection,
        "agent-child",
        "conversation-child",
        "child-custom",
        custom_parent,
        30,
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT schema_version FROM agent_effective_permission_snapshots WHERE agent_id = 'agent-child'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        i64::from(AGENT_EFFECTIVE_PERMISSION_SNAPSHOT_SCHEMA_VERSION)
    );

    let inherited = inherit_agent_permissions_in_transaction(&connection, "agent-grand")
        .expect("the direct parent snapshot should be the initial authority");
    assert_eq!(inherited, custom_parent);

    // Root tightens while the intermediate child remains idle. A direct follow-up of the
    // grandchild must not inherit the child's now-stale broader snapshot.
    record_permissions_for_test_turn(
        &mut connection,
        "agent-root",
        "conversation-root",
        "root-tightened",
        AgentPermissions::default(),
        40,
    );
    let tightened = inherit_agent_permissions_in_transaction(&connection, "agent-grand")
        .expect("the complete ancestor chain should remain available");
    assert_eq!(tightened, AgentPermissions::default());
    let followup = follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-grand".to_string(),
            request_id: "root-direct-grand-tightened".to_string(),
            content: "Run only with the root's newly tightened authority.".to_string(),
        },
        50,
    )
    .unwrap();
    let claimed = claim_next_dispatchable_agent_wake(
        &mut connection,
        "claim-root-direct-grand-tightened",
        51,
    )
    .unwrap()
    .unwrap();
    assert_eq!(claimed.agent_id, "agent-grand");
    assert_eq!(
        claimed.source_agent_message_id,
        Some(followup.message.message_id)
    );
    assert_eq!(
        inherit_agent_permissions_in_transaction(&connection, "agent-grand").unwrap(),
        AgentPermissions::default()
    );
    assert_eq!(
        get_agent_effective_permission_snapshot(&connection, "agent-root")
            .unwrap()
            .unwrap()
            .revision,
        2
    );
}

#[test]
fn permission_inheritance_and_active_turn_recording_fail_closed_on_missing_or_forged_facts() {
    let mut connection = setup_tree();
    let missing = inherit_agent_permissions_in_transaction(&connection, "agent-child").unwrap_err();
    assert!(
        missing
            .to_string()
            .contains("missing a durable ancestor snapshot"),
        "{missing}"
    );

    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'assistant-forged-permissions', 'conversation-root', 'assistant',
                     'permission fixture', 'pending', 20, 0
                 )",
            [],
        )
        .unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-forged-permissions",
        "conversation-root",
        "assistant-forged-permissions",
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        &mut connection,
        &trace,
        20,
        20,
    )
    .unwrap();
    let forged = record_agent_effective_permissions_for_active_turn(
        &mut connection,
        "agent-child",
        "conversation-root",
        "run-forged-permissions",
        "assistant-forged-permissions",
        full_permissions(),
        20,
    )
    .unwrap_err();
    assert!(
        forged.to_string().contains("bound Conversation"),
        "{forged}"
    );
    assert!(
        get_agent_effective_permission_snapshot(&connection, "agent-child")
            .unwrap()
            .is_none()
    );
}
