use super::*;

fn competing_roots() -> Connection {
    let mut connection = setup_tree();
    add_grandchild(&mut connection);
    for id in ["conversation-other-root", "conversation-other-child"] {
        insert_conversation(&connection, id, Some("project-a"));
    }
    ensure_root(&mut connection, "other-root", "conversation-other-root");
    create_agent_node(
        &mut connection,
        &child_input(
            "other-child",
            "other-root",
            "other-root",
            "conversation-other-child",
            "other",
            "/root/other",
        ),
        12,
    )
    .unwrap();
    for (suffix, root, agent, parent) in [
        ("a-first", "agent-root", "agent-child", "agent-root"),
        ("a-next", "agent-root", "agent-child", "agent-root"),
        ("a-grand", "agent-root", "agent-grand", "agent-child"),
        ("b-first", "other-root", "other-child", "other-root"),
    ] {
        enqueue_agent_wake(
            &mut connection,
            &EnqueueAgentWakeInput {
                wake_id: format!("wake-{suffix}"),
                root_agent_id: root.into(),
                agent_id: agent.into(),
                requester_agent_id: parent.into(),
                request_id: format!("request-{suffix}"),
                source_agent_message_id: None,
            },
            20,
        )
        .unwrap();
    }
    connection
}

#[test]
fn fair_claim_skips_preparing_roots_but_preserves_agent_fifo_and_claim_replay() {
    let mut connection = competing_roots();
    let a = claim_next_fair_dispatchable_agent_wake(&mut connection, "claim-a", 21)
        .unwrap()
        .unwrap();
    assert_eq!(a.wake_id, "wake-a-first");
    assert_eq!(
        claim_next_fair_dispatchable_agent_wake(&mut connection, "claim-a", 22)
            .unwrap()
            .unwrap()
            .wake_id,
        a.wake_id,
        "an acknowledged claim remains idempotent despite its root preparation fence",
    );
    let b = claim_next_fair_dispatchable_agent_wake(&mut connection, "claim-b", 22)
        .unwrap()
        .unwrap();
    assert_eq!(b.wake_id, "wake-b-first");
    assert!(
        claim_next_fair_dispatchable_agent_wake(&mut connection, "claim-none", 23)
            .unwrap()
            .is_none()
    );
    transition_agent_wake(
        &mut connection,
        &a.wake_id,
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Running,
        Some("claim-a"),
        24,
    )
    .unwrap();
    let sibling = claim_next_fair_dispatchable_agent_wake(&mut connection, "claim-grand", 25)
        .unwrap()
        .unwrap();
    assert_eq!(
        sibling.wake_id, "wake-a-grand",
        "running siblings do not consume preparation capacity"
    );
    assert_eq!(
        get_agent_wake(&connection, "wake-a-next")
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Queued
    );
    transition_agent_wake(
        &mut connection,
        &sibling.wake_id,
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Cancelled,
        Some("claim-grand"),
        26,
    )
    .unwrap();
    transition_agent_wake(
        &mut connection,
        &a.wake_id,
        AgentWakeStatus::Running,
        AgentWakeStatus::Cancelled,
        Some("claim-a"),
        26,
    )
    .unwrap();
    assert_eq!(
        claim_next_fair_dispatchable_agent_wake(&mut connection, "claim-next", 27)
            .unwrap()
            .unwrap()
            .wake_id,
        "wake-a-next"
    );
}

#[test]
fn legacy_claim_retains_global_fifo_while_fair_claim_waits_for_expired_owner_recovery() {
    let mut connection = competing_roots();
    let a = claim_next_fair_dispatchable_agent_wake(&mut connection, "crashed-a", 21)
        .unwrap()
        .unwrap();
    // A legacy caller retains its original behavior; the new scheduling rule is opt-in.
    let sibling = claim_next_dispatchable_agent_wake(&mut connection, "legacy-sibling", 22)
        .unwrap()
        .unwrap();
    assert_eq!(sibling.wake_id, "wake-a-grand");
    let b = claim_next_fair_dispatchable_agent_wake(&mut connection, "fair-b", 23)
        .unwrap()
        .unwrap();
    assert_eq!(b.wake_id, "wake-b-first");
    let expires_at = a
        .lease_expires_at
        .unwrap()
        .max(sibling.lease_expires_at.unwrap())
        .max(b.lease_expires_at.unwrap());
    assert!(
        claim_next_fair_dispatchable_agent_wake(&mut connection, "before-recovery", expires_at)
            .unwrap()
            .is_none(),
        "expiry alone never steals an unreconciled preparation"
    );
    recover_agent_wakes(&mut connection, "recover-fair-roots", expires_at).unwrap();
    assert_eq!(
        claim_next_fair_dispatchable_agent_wake(&mut connection, "after-recovery", expires_at + 1)
            .unwrap()
            .unwrap()
            .wake_id,
        "wake-a-first"
    );
}
