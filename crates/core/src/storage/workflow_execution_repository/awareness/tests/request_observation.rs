use super::*;

fn assert_equivalent(c: &Connection, chat: &str, run: &str) {
    let identity = snapshot_for_run(c, chat, run).unwrap().unwrap();
    let full = awareness_for_run(c, chat, run).unwrap();
    let observation = request_observation(c, chat, Some(run)).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&observation.0).unwrap(),
        serde_json::to_value(identity).unwrap()
    );
    for key in [
        "available",
        "instanceId",
        "executionVersion",
        "organizationRevision",
        "currentNodeId",
        "mailbox",
    ] {
        assert_eq!(
            observation.1[key], full[key],
            "changed automatic field: {key}"
        );
    }
    assert!(observation.1.get("nodes").is_none());
    assert!(observation.1.get("recentSent").is_none());
}

#[test]
fn automatic_request_observation_preserves_identity_and_mailbox_across_mail_states() {
    let mut c = fixture();
    let receipt = send_mail(
        &mut c,
        "request-mail",
        &[("b", "first body"), ("b", "second body")],
    );
    start_run(&mut c, "b", "run-b");
    let chat = conversation(&c, "b");
    assert_equivalent(&c, &chat, "run-b");
    let accept = action(
        &c,
        "b",
        "run-b",
        "accept",
        MutationAction::Accept,
        &[receipt.messages[0].id.clone()],
    );
    mutate(&mut c, &accept).unwrap();
    assert_equivalent(&c, &chat, "run-b");
    prove(&c, &receipt.input_ids[0]);
    let complete = action(
        &c,
        "b",
        "run-b",
        "complete",
        MutationAction::Complete,
        &[receipt.messages[0].id.clone()],
    );
    mutate(&mut c, &complete).unwrap();
    assert_equivalent(&c, &chat, "run-b");
    assert_eq!(
        load_input(&c, &receipt.input_ids[1])
            .unwrap()
            .unwrap()
            .mail_status,
        MailStatus::Pending
    );
}

#[test]
fn automatic_request_observation_has_constant_queries_and_builds_one_graph() {
    let mut counts = Vec::new();
    for member_count in [3, 128] {
        let mut c = fixture();
        let mut definition: Value = serde_json::from_str(
            &c.query_row(
                "SELECT definition_json FROM workflow_instances WHERE instance_id='instance'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        )
        .unwrap();
        let prototype = definition["nodes"][0].clone();
        for index in 3..member_count {
            let mut node = prototype.clone();
            node["id"] = value!(format!("member-{index}"));
            node["name"] = value!(format!("Member {index}"));
            definition["nodes"].as_array_mut().unwrap().push(node);
        }
        c.execute(
            "UPDATE workflow_instances SET definition_json=?1 WHERE instance_id='instance'",
            [definition.to_string()],
        )
        .unwrap();
        let chat = conversation(&c, "a");
        SUMMARY_SQL.with(|statements| statements.borrow_mut().clear());
        c.trace(Some(record_summary_sql));
        let observed = request_observation(&c, &chat, Some("run-a"))
            .unwrap()
            .unwrap();
        c.trace(None);
        let statements = SUMMARY_SQL.with(|statements| statements.borrow().clone());
        assert_eq!(observed.0.members.len(), member_count);
        assert_eq!(
            statements
                .iter()
                .filter(|sql| sql.contains("SELECT name,definition_json"))
                .count(),
            1
        );
        assert!(!statements
            .iter()
            .any(|sql| sql.contains("conversation_turn_traces")
                || sql.contains("human_interaction")
                || sql.contains("agent_pending_actions")));
        assert!(
            statements.len() <= 10,
            "unbounded automatic observation: {statements:?}"
        );
        counts.push(statements.len());
    }
    assert_eq!(counts[0], counts[1]);
}

#[test]
fn automatic_request_observation_does_not_restore_disabled_or_rebound_run_authority() {
    let c = fixture();
    let chat = conversation(&c, "a");
    assert!(request_observation(&c, &chat, Some("missing-run"))
        .unwrap()
        .is_none());
    assert!(request_observation(&c, &chat, None).unwrap().is_some());
    c.execute(
        "UPDATE workflow_instances SET enabled=0 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    assert!(request_observation(&c, &chat, Some("run-a"))
        .unwrap()
        .is_none());
    c.execute(
        "UPDATE workflow_instances SET enabled=1 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    c.execute("UPDATE workflow_instance_bindings SET membership_id='replacement-membership' WHERE node_id='a'", []).unwrap();
    assert!(request_observation(&c, &chat, Some("run-a"))
        .unwrap()
        .is_none());
    assert!(request_observation(&c, &chat, None).unwrap().is_some());
}
