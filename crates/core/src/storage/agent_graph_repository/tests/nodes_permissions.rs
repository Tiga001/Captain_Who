use super::*;

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
