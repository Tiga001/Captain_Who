use super::*;
use crate::storage::workflow_execution_repository::tests::{
    action, conversation, fixture, prove, send_mail, start_run,
};

#[test]
fn workflow_awareness_pending_preview_is_read_only_and_arrival_counts_are_monotonic() {
    let mut c = fixture();
    let receipt = send_mail(
        &mut c,
        "send",
        &[("b", "pending body"), ("b", "also waiting")],
    );
    start_run(&mut c, "b", "run-b");
    let chat = conversation(&c, "b");
    let inbox = mailbox_for_run(&c, &chat, "run-b", &MailboxQuery::default()).unwrap();
    assert_eq!(inbox["messages"][0]["content"], "also waiting");
    assert_eq!(inbox["messages"][0]["status"], "pending");
    assert_eq!(inbox["messages"][0]["bodyAvailable"], true);
    let before = awareness_for_run(&c, &chat, "run-b").unwrap();
    assert_eq!(before["mailbox"]["receivedCount"], 2);
    assert_eq!(before["mailbox"]["pendingCount"], 2);
    assert_eq!(before["mailbox"]["processingCount"], 0);
    assert_eq!(
        load_input(&c, &receipt.input_ids[0])
            .unwrap()
            .unwrap()
            .mail_status,
        MailStatus::Pending
    );
    let accept = action(
        &c,
        "b",
        "run-b",
        "accept",
        MutationAction::Accept,
        &[receipt.messages[0].id.clone()],
    );
    mutate(&mut c, &accept).unwrap();
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
    let after = awareness_for_run(&c, &chat, "run-b").unwrap();
    assert_eq!(after["mailbox"]["receivedCount"], 2);
    assert_eq!(after["mailbox"]["pendingCount"], 1);
    assert_eq!(
        after["mailbox"]["latestSequence"],
        before["mailbox"]["latestSequence"]
    );
    assert_eq!(
        after["mailbox"]["recentArrivals"].as_array().unwrap().len(),
        2
    );
}
#[test]
fn workflow_awareness_mailbox_status_filter_pagination_and_body_budget_are_explicit() {
    let mut c = fixture();
    let text = "a".repeat(100_000);
    let receipt = send_mail(&mut c, "send", &[("b", &text), ("b", &text), ("b", &text)]);
    start_run(&mut c, "b", "run-b");
    let chat = conversation(&c, "b");
    let all = mailbox_for_run(&c, &chat, "run-b", &MailboxQuery::default()).unwrap();
    assert_eq!(all["messages"][2]["bodyAvailable"], false);
    assert!(all["messages"][2].get("content").is_none());
    let focused = MailboxQuery {
        message_id: Some(receipt.messages[0].id.clone()),
        ..Default::default()
    };
    assert_eq!(
        mailbox_for_run(&c, &chat, "run-b", &focused).unwrap()["messages"][0]["content"],
        text
    );
    let page = mailbox_for_run(
        &c,
        &chat,
        "run-b",
        &MailboxQuery {
            limit: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let next = mailbox_for_run(
        &c,
        &chat,
        "run-b",
        &MailboxQuery {
            limit: 1,
            cursor: page["nextCursor"].as_u64(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_ne!(
        page["messages"][0]["messageId"],
        next["messages"][0]["messageId"]
    );
    let recalled = action(
        &c,
        "a",
        "run-a",
        "recall",
        MutationAction::Recall,
        &[receipt.messages[0].id.clone()],
    );
    mutate(&mut c, &recalled).unwrap();
    let filter = MailboxQuery {
        status: Some(MailStatus::Recalled),
        ..Default::default()
    };
    let messages = mailbox_for_run(&c, &chat, "run-b", &filter).unwrap();
    assert_eq!(messages["messages"].as_array().unwrap().len(), 1);
    assert_eq!(messages["messages"][0]["status"], "recalled");
}
#[test]
fn workflow_awareness_mailbox_ownership_is_frozen_to_real_sender_and_recipient() {
    let mut c = fixture();
    let receipt = send_mail(&mut c, "send", &[("b", "private")]);
    start_run(&mut c, "c", "run-c");
    let chat = conversation(&c, "c");
    let query = MailboxQuery {
        message_id: Some(receipt.messages[0].id.clone()),
        ..Default::default()
    };
    assert!(
        mailbox_for_run(&c, &chat, "run-c", &query).unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(mailbox_for_run(&c, &chat, "run-a", &query).is_err());
    let sender = conversation(&c, "a");
    let outbox = MailboxQuery {
        direction: MailboxDirection::Outbox,
        ..Default::default()
    };
    assert_eq!(
        mailbox_for_run(&c, &sender, "run-a", &outbox).unwrap()["messages"][0]["targetNodeName"],
        "b"
    );
}
#[test]
fn workflow_awareness_member_snapshot_is_small_but_focused_query_has_complete_roles() {
    let c = fixture();
    let mut definition: Value = serde_json::from_str(
        &c.query_row(
            "SELECT definition_json FROM workflow_instances WHERE instance_id='instance'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    definition["nodes"][1]["task"] = value!("x".repeat(2000));
    c.execute(
        "UPDATE workflow_instances SET definition_json=?1 WHERE instance_id='instance'",
        [definition.to_string()],
    )
    .unwrap();
    let chat = conversation(&c, "a");
    let snapshot = snapshot_for_conversation(&c, &chat).unwrap().unwrap();
    assert_eq!(snapshot.members[1].task.len(), 512);
    let all = state_for_run(
        &c,
        &chat,
        "run-a",
        &StateQuery {
            view: StateView::Members,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(all["members"][1]["task"].as_str().unwrap().len(), 512);
    assert!(all.get("background").is_none());
    assert!(all.get("topology").is_none());
    let focused = state_for_run(
        &c,
        &chat,
        "run-a",
        &StateQuery {
            node_id: Some("b".into()),
            view: StateView::Members,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(focused["members"][0]["task"].as_str().unwrap().len(), 2000);
    assert!(focused.get("runtime").is_none());
}
#[test]
fn workflow_awareness_monitor_uses_mail_state_without_bodies_or_terminal_override() {
    let mut c = fixture();
    let receipt = send_mail(&mut c, "send", &[("b", "private body")]);
    start_run(&mut c, "b", "run-b");
    let accept = action(
        &c,
        "b",
        "run-b",
        "accept",
        MutationAction::Accept,
        &[receipt.messages[0].id.clone()],
    );
    mutate(&mut c, &accept).unwrap();
    prove(&c, &receipt.input_ids[0]);
    settle_run(&c, "run-b", "completed").unwrap();
    c.execute(
        "UPDATE conversation_turn_traces SET terminal_status='cancelled',completed_at=2 WHERE run_id='run-b'",
        [],
    )
    .unwrap();
    let snapshot = runtime_snapshot(&c, "instance", None).unwrap();
    assert!(snapshot.inputs[0].content.is_empty());
    assert!(snapshot.inputs[0].messages[0].content.is_empty());
    assert_eq!(snapshot.inputs[0].mail_status, MailStatus::Processed);
    let messages = node_messages(&c, "instance", "b", None).unwrap();
    assert_eq!(messages.messages[0].status, "processed");
    assert_eq!(messages.messages[0].message.content, "private body");
    assert_eq!(
        messages.messages[0].run_status.as_deref(),
        Some("cancelled")
    );
}
#[test]
fn workflow_awareness_backlog_does_not_hide_mail_accepted_by_current_turn() {
    let mut c = fixture();
    let messages = vec![("b", "waiting"); 128];
    send_mail(&mut c, "backlog", &messages);
    let receipt = send_mail(&mut c, "latest", &[("b", "handle this now")]);
    start_run(&mut c, "b", "run-b");
    let accept = action(
        &c,
        "b",
        "run-b",
        "accept",
        MutationAction::Accept,
        &[receipt.messages[0].id.clone()],
    );
    mutate(&mut c, &accept).unwrap();
    let summary = awareness_for_run(&c, &conversation(&c, "b"), "run-b").unwrap();
    assert_eq!(summary["currentInputCount"], 1);
    assert_eq!(summary["currentInputIds"], value!([receipt.input_ids[0]]));
    assert_eq!(summary["mailbox"]["pendingCount"], 128);
}

#[test]
fn configuration_is_explicit_scoped_and_distinguishes_defaults_from_next_turn() {
    let mut c = fixture();
    let chat = conversation(&c, "a");
    let query = StateQuery {
        view: StateView::Configuration,
        node_id: Some("b".into()),
        ..Default::default()
    };
    assert!(state_for_run(&c, &chat, "run-a", &query).is_err());
    let mut definition: Value = serde_json::from_str(
        &c.query_row(
            "SELECT definition_json FROM workflow_instances WHERE instance_id='instance'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    definition["nodes"][0]["rank"] = value!(99);
    definition["nodes"][0]["managementRole"] = value!("organization_admin");
    c.execute(
        "UPDATE workflow_instances SET definition_json=?1 WHERE instance_id='instance'",
        [definition.to_string()],
    )
    .unwrap();
    let target = conversation(&c, "b");
    crate::storage::composer_draft_repository::set_composer_configuration(
        &c,
        &target,
        Some("next-model"),
        "custom",
        10,
    )
    .unwrap();
    start_run(&mut c, "b", "run-b");
    let result = state_for_run(&c, &chat, "run-a", &query).unwrap();
    let member = &result["configuration"]["members"][0];
    assert_eq!(member["memberDefaults"]["modelConfigId"], "model");
    assert_eq!(member["nextTurn"]["modelConfigId"], "next-model");
    assert_eq!(member["nextTurn"]["permissionMode"], "custom");
    assert_eq!(member["activeRun"]["runId"], "run-b");
    assert!(member["activeRun"].get("modelConfigId").is_none());
    assert!(result.get("runtime").is_none());
    assert!(state_for_run(&c, &chat, "run-a", &StateQuery::default())
        .unwrap()
        .get("configuration")
        .is_none());
    assert!(awareness_for_run(&c, &chat, "run-a")
        .unwrap()
        .get("configuration")
        .is_none());

    // A department administrator cannot inspect a member outside the department or a peer.
    definition["departments"] =
        value!([{"id":"team","name":"Team","parentId":null,"x":0,"y":0,"width":400,"height":300}]);
    definition["nodes"][0]["managementRole"] = value!("department_admin");
    definition["nodes"][0]["departmentId"] = value!("team");
    c.execute(
        "UPDATE workflow_instances SET definition_json=?1 WHERE instance_id='instance'",
        [definition.to_string()],
    )
    .unwrap();
    assert!(state_for_run(&c, &chat, "run-a", &query).is_err());
    definition["nodes"][1]["departmentId"] = value!("team");
    definition["nodes"][1]["rank"] = value!(99);
    c.execute(
        "UPDATE workflow_instances SET definition_json=?1 WHERE instance_id='instance'",
        [definition.to_string()],
    )
    .unwrap();
    assert!(state_for_run(&c, &chat, "run-a", &query).is_err());
    let own = state_for_run(
        &c,
        &chat,
        "run-a",
        &StateQuery {
            node_id: Some("a".into()),
            ..query
        },
    )
    .unwrap();
    assert_eq!(own["configuration"]["members"][0]["editable"], false);
}

thread_local! {
    static SUMMARY_SQL: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}
fn record_summary_sql(sql: &str) {
    SUMMARY_SQL.with(|statements| statements.borrow_mut().push(sql.to_owned()));
}

#[test]
fn automatic_summary_scales_without_loading_every_members_mail_details() {
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
    for index in 3..128 {
        let mut node = prototype.clone();
        node["id"] = value!(format!("member-{index}"));
        node["name"] = value!(format!("Member {index}"));
        definition["nodes"].as_array_mut().unwrap().push(node);
    }
    crate::storage::workflow_repository::request(&mut c,
        serde_json::from_value(value!({"operation":"saveInstance","id":"instance","name":"Team","color":"#123456","definition":definition,"bindings":[],"expectedRevision":1})).unwrap(),
        &std::collections::HashSet::from(["model".into()])).unwrap();
    send_mail(
        &mut c,
        "summary-mail",
        &[("b", "private"), ("b", "waiting")],
    );
    let chat = conversation(&c, "a");
    SUMMARY_SQL.with(|statements| statements.borrow_mut().clear());
    c.trace(Some(record_summary_sql));
    let summary = awareness_for_run(&c, &chat, "run-a").unwrap();
    c.trace(None);
    let statements = SUMMARY_SQL.with(|statements| statements.borrow().clone());
    assert_eq!(summary["totalNodeCount"], 128);
    assert_eq!(summary["nodes"].as_array().unwrap().len(), 16);
    assert_eq!(summary["nodes"][0]["nodeId"], "a");
    assert_eq!(summary["nodes"][1]["pendingCount"], 2);
    assert!(summary["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|node| node.get("inputs").is_none()));
    assert!(
        statements.len() < 25,
        "automatic summary used {} SQL statements",
        statements.len()
    );
    assert!(!statements
        .iter()
        .any(|sql| sql.contains("SELECT i.input_id,m.message_id")));
}

fn organized_fixture() -> Connection {
    let c = fixture();
    let mut definition: Value = serde_json::from_str(
        &c.query_row(
            "SELECT definition_json FROM workflow_instances WHERE instance_id='instance'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    definition["departments"] = value!([
        {"id":"research","name":"Research","parentId":null,"x":0,"y":0,"width":400,"height":300},
        {"id":"applied","name":"Applied","parentId":"research","x":0,"y":0,"width":400,"height":300},
        {"id":"outside","name":"Operations","parentId":null,"x":0,"y":0,"width":400,"height":300}
    ]);
    definition["nodes"][0]["name"] = value!("Alice");
    definition["nodes"][0]["departmentId"] = value!("research");
    definition["nodes"][1]["name"] = value!("Bob");
    definition["nodes"][1]["task"] = value!("Extract catalyst mechanisms from literature");
    definition["nodes"][1]["departmentId"] = value!("applied");
    definition["nodes"][2]["name"] = value!("Cara");
    definition["nodes"][2]["departmentId"] = value!("outside");
    c.execute(
        "UPDATE workflow_instances SET definition_json=?1 WHERE instance_id='instance'",
        [definition.to_string()],
    )
    .unwrap();
    c
}

fn modeled_state(c: &Connection, query: &StateQuery) -> Value {
    crate::workflow_awareness::state_for_model(
        state_for_run(c, &conversation(c, "a"), "run-a", query).unwrap(),
        query,
    )
}

#[test]
fn organization_state_members_search_department_descendants_and_full_own_contract_are_explicit() {
    let c = organized_fixture();
    let query = StateQuery {
        view: StateView::Members,
        department_id: Some("research".into()),
        limit: 1,
        ..Default::default()
    };
    let first = modeled_state(&c, &query);
    assert_eq!(first["page"]["total"], 2);
    assert_eq!(first["page"]["nextCursor"], 1);
    assert_eq!(first["members"][0]["member"], "Alice");
    assert_eq!(first["scope"]["department"], "Research");
    assert_eq!(
        first["members"][0]["responsibilitiesAvailability"],
        "summary"
    );
    assert!(first["members"][0].get("receives").is_none());
    let next = modeled_state(
        &c,
        &StateQuery {
            cursor: Some(1),
            ..query.clone()
        },
    );
    assert_eq!(next["members"][0]["member"], "Bob");
    assert_eq!(next["members"][0]["department"], "Research/Applied");
    let direct = modeled_state(
        &c,
        &StateQuery {
            include_descendants: false,
            ..query.clone()
        },
    );
    assert_eq!(direct["page"]["total"], 1);
    let keyword = modeled_state(
        &c,
        &StateQuery {
            search: Some("CATALYST".into()),
            ..query.clone()
        },
    );
    assert_eq!(keyword["page"]["total"], 1);
    assert_eq!(keyword["members"][0]["member"], "Bob");
    let missing = modeled_state(
        &c,
        &StateQuery {
            search: Some("Not found".into()),
            ..query.clone()
        },
    );
    assert_eq!(missing["members"], value!([]));
    assert_eq!(missing["page"]["total"], 0);
    let own = modeled_state(
        &c,
        &StateQuery {
            node_id: Some("a".into()),
            ..query
        },
    );
    assert_eq!(own["members"][0]["receives"], "Artifacts");
    assert_eq!(own["members"][0]["task"], "Review quality");
    assert_eq!(own["members"][0]["delivers"], "Review report");
    assert_eq!(
        own["members"][0]["responsibilitiesAvailability"],
        "complete"
    );
    assert!(!own.to_string().contains("conversationId"));
}

#[test]
fn organization_state_structure_pages_keep_parent_paths_and_subtree_counts() {
    let c = organized_fixture();
    let query = StateQuery {
        view: StateView::Structure,
        department_id: Some("research".into()),
        limit: 1,
        ..Default::default()
    };
    let first = modeled_state(&c, &query);
    assert_eq!(first["page"]["total"], 2);
    assert_eq!(
        first["structure"]["departments"][0]["department"],
        "Research"
    );
    assert_eq!(first["structure"]["departments"][0]["directMemberCount"], 1);
    assert_eq!(first["structure"]["departments"][0]["memberCount"], 2);
    assert_eq!(
        first["structure"]["departments"][0]["childDepartments"],
        value!(["Research/Applied"])
    );
    let second = modeled_state(
        &c,
        &StateQuery {
            cursor: Some(1),
            ..query
        },
    );
    assert_eq!(
        second["structure"]["departments"][0]["department"],
        "Research/Applied"
    );
    assert_eq!(
        second["structure"]["departments"][0]["parentDepartment"],
        "Research"
    );
    assert_eq!(second["structure"]["departments"][0]["depth"], 1);
    assert_eq!(second["structure"]["departments"][0]["memberCount"], 1);
    assert!(second["page"]["nextCursor"].is_null());
    assert!(second.get("_directory").is_none());
    assert!(state_for_run(
        &c,
        &conversation(&c, "a"),
        "run-a",
        &StateQuery {
            department_id: Some("not-a-department".into()),
            ..Default::default()
        }
    )
    .is_err());
}

#[test]
fn organization_state_runtime_candidates_are_not_paged_before_host_and_mail_is_opt_in_with_stable_cursor(
) {
    let mut c = organized_fixture();
    let receipt = send_mail(
        &mut c,
        "three-letters",
        &[("b", "first"), ("b", "second"), ("b", "third")],
    );
    start_run(&mut c, "b", "run-b");
    let accept = action(
        &c,
        "b",
        "run-b",
        "accept-second",
        MutationAction::Accept,
        &[receipt.messages[1].id.clone()],
    );
    mutate(&mut c, &accept).unwrap();
    let query = StateQuery {
        view: StateView::Runtime,
        limit: 1,
        ..Default::default()
    };
    let mut raw = state_for_run(&c, &conversation(&c, "a"), "run-a", &query).unwrap();
    assert_eq!(raw["runtime"]["nodes"].as_array().unwrap().len(), 3);
    assert!(raw["runtime"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|node| node.get("inputs").is_none()));
    raw["runtime"]["nodes"][1]["state"] = value!("waiting_interaction");
    raw["runtime"]["nodes"][1]["waitingForInteraction"] = value!(true);
    raw["runtime"]["nodes"][1]["hasPendingInteraction"] = value!(true);
    let filtered = crate::workflow_awareness::state_for_model(
        raw,
        &StateQuery {
            status: Some("waiting_interaction".into()),
            ..query.clone()
        },
    );
    assert_eq!(filtered["page"]["total"], 1);
    assert_eq!(filtered["runtime"]["members"][0]["member"], "Bob");
    assert_eq!(
        filtered["runtime"]["members"][0]["waitingForInteraction"],
        true
    );
    assert_eq!(
        filtered["runtime"]["members"][0]["hasPendingInteraction"],
        true
    );
    assert_eq!(filtered["runtime"]["members"][0]["pendingCount"], 2);
    assert_eq!(filtered["runtime"]["members"][0]["processingCount"], 1);
    assert_eq!(
        filtered["runtime"]["members"][0]["mailCounts"]["processed"],
        0
    );
    assert_eq!(
        filtered["runtime"]["members"][0]["mailAvailability"],
        "not_requested"
    );
    let focused = StateQuery {
        node_id: Some("b".into()),
        include_mail: true,
        ..query
    };
    let first = modeled_state(&c, &focused);
    let member = &first["runtime"]["members"][0];
    assert_eq!(member["mailPage"]["total"], 3);
    assert_eq!(member["mailPage"]["returned"], 1);
    assert_eq!(member["mailPage"]["truncated"], true);
    assert_eq!(member["mail"][0]["messageId"], receipt.messages[0].id);
    let second = modeled_state(
        &c,
        &StateQuery {
            mail_cursor: member["mailPage"]["nextCursor"].as_u64(),
            ..focused.clone()
        },
    );
    let member = &second["runtime"]["members"][0];
    assert_eq!(member["mail"][0]["messageId"], receipt.messages[1].id);
    assert_eq!(member["mail"][0]["status"], "processing");
    assert!(member["mail"][0].get("content").is_none());
    let third = modeled_state(
        &c,
        &StateQuery {
            mail_cursor: member["mailPage"]["nextCursor"].as_u64(),
            ..focused
        },
    );
    assert_eq!(
        third["runtime"]["members"][0]["mail"][0]["messageId"],
        receipt.messages[2].id
    );
    assert!(third["runtime"]["members"][0]["mailPage"]["nextCursor"].is_null());
}

#[test]
fn organization_state_configuration_pages_only_authorized_filtered_members() {
    let c = organized_fixture();
    c.execute("UPDATE workflow_instances SET definition_json=json_set(definition_json,'$.nodes[0].rank',99,'$.nodes[0].managementRole','department_admin') WHERE instance_id='instance'",[]).unwrap();
    let query = StateQuery {
        view: StateView::Configuration,
        limit: 1,
        ..Default::default()
    };
    let first = modeled_state(&c, &query);
    assert_eq!(
        first["configuration"]["access"]["scope"],
        "authorized_members_only"
    );
    assert_eq!(first["configuration"]["access"]["excludedMemberCount"], 1);
    assert_eq!(first["page"]["total"], 2);
    assert_eq!(first["page"]["nextCursor"], 1);
    let next = modeled_state(
        &c,
        &StateQuery {
            cursor: Some(1),
            ..query.clone()
        },
    );
    assert_eq!(next["configuration"]["members"][0]["member"], "Bob");
    assert_eq!(
        next["configuration"]["members"][0]["department"],
        "Research/Applied"
    );
    let search = modeled_state(
        &c,
        &StateQuery {
            search: Some("catalyst".into()),
            ..query.clone()
        },
    );
    assert_eq!(search["page"]["total"], 1);
    assert_eq!(search["configuration"]["members"][0]["member"], "Bob");
    let empty = modeled_state(
        &c,
        &StateQuery {
            department_id: Some("outside".into()),
            ..query.clone()
        },
    );
    assert_eq!(empty["page"]["total"], 0);
    assert_eq!(empty["configuration"]["access"]["excludedMemberCount"], 1);
    let denied = state_for_run(
        &c,
        &conversation(&c, "a"),
        "run-a",
        &StateQuery {
            node_id: Some("c".into()),
            ..query
        },
    )
    .unwrap_err();
    assert!(denied.contains("organization_configuration_forbidden"));
}
