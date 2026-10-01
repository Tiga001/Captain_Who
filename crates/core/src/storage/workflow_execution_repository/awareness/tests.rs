use super::*;
use crate::storage::migrations::run_migrations;

fn fixture() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    let agent = |id: &str| json!({"kind":"agent","id":id,"name":id,"x":0,"y":0,"permissionMode":"default","modelConfigId":"model","receives":"Artifacts","task":"Review quality","delivers":"Report"});
    let flow = |id: &str, source: Option<&str>, target: &str| json!({"id":id,"name":id,"source":source.map(|id|json!({"kind":"node","nodeId":id})).unwrap_or(json!({"kind":"boundary"})),"target":{"kind":"node","nodeId":target}});
    let definition = json!({"schemaVersion":1,"id":"template","name":"Template","description":"Shared description","background":"Shared background",
        "nodes":[agent("a"),agent("b"),
            {"kind":"user","id":"human","name":"Human","x":0,"y":0,"task":"Accept release"},
            {"kind":"inputGate","id":"entry-gate","name":"Entry gate","x":0,"y":0,"processingMode":"individual","busyPolicy":"queue"},
            {"kind":"inputGate","id":"batch-gate","name":"Batch gate","x":0,"y":0,"processingMode":"batch","busyPolicy":"inject"},
            {"kind":"outputGate","id":"out","name":"Output","x":0,"y":0,"selection":{"mode":"any","min":1,"max":3,"required":[],"groups":[]}}],
        "flows":[flow("entry",None,"entry-gate"),flow("entry-bind",Some("entry-gate"),"a"),flow("loop",Some("b"),"entry-gate"),flow("out-bind",Some("a"),"out"),flow("left",Some("out"),"batch-gate"),flow("right",Some("out"),"batch-gate"),flow("batch-bind",Some("batch-gate"),"b"),flow("human-flow",Some("out"),"human")],
        "viewport":{"x":0,"y":0,"zoom":1},"boundaryPositions":{"input":{"x":0,"y":0}}});
    let models = std::collections::HashSet::from(["model".into()]);
    crate::storage::workflow_repository::request(
        &mut c,
        serde_json::from_value(
            json!({"operation":"save","definition":definition,"expectedRevision":0}),
        )
        .unwrap(),
        &models,
    )
    .unwrap();
    crate::storage::workflow_repository::request(&mut c, serde_json::from_value(json!({"operation":"saveInstance","id":"instance","templateId":"template","name":"Review workflow","color":"#123456","bindings":[{"nodeId":"a","conversationId":null},{"nodeId":"b","conversationId":null}],"expectedRevision":0,"expectedTemplateRevision":1})).unwrap(), &models).unwrap();
    for node in ["a", "b"] {
        let chat = conversation(&c, node);
        add_run(&mut c, &chat, node);
    }
    c
}

fn conversation(c: &Connection, node: &str) -> String {
    c.query_row("SELECT conversation_id FROM workflow_instance_bindings WHERE instance_id='instance' AND node_id=?1",[node],|row|row.get(0)).unwrap()
}

fn add_run(c: &mut Connection, chat: &str, run: &str) {
    let assistant = format!("assistant-{run}");
    c.execute("INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES (?1,?2,'assistant','Private chat text',1,0)",params![assistant,chat]).unwrap();
    c.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES (?1,?2,?3,1,'in_progress',0,1,1)",params![assistant,chat,run]).unwrap();
    bind_run(c, chat, run).unwrap();
}

fn send_message(c: &mut Connection, call: &str, flow: &str, content: &str) -> SendReceipt {
    let chat = conversation(c, "a");
    let identity = snapshot_for_run(c, &chat, "a").unwrap().unwrap();
    send(
        c,
        &SendRequest {
            conversation_id: chat,
            source_run_id: "a".into(),
            tool_call_id: call.into(),
            execution_version: identity.execution_version,
            outputs: vec![SendOutput {
                flow_id: flow.into(),
                message: content.into(),
            }],
        },
    )
    .unwrap()
}

fn mailbox(c: &Connection, node: &str, query: MailboxQuery) -> Value {
    mailbox_for_run(c, &conversation(c, node), node, &query).unwrap()
}

fn receiver(c: &Connection) -> Value {
    mailbox(c, "b", MailboxQuery::default())
}

fn prove_delivery(c: &mut Connection, id: &str) {
    let input = load_input(c, id).unwrap().unwrap();
    let proof = json!({"type":"workflow_delivery","sequence":0,"inputId":id,"instanceId":input.instance_id,"workflowName":"Review workflow","content":input.content,"createdAt":input.created_at,"truncated":false});
    c.execute("INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json) VALUES ('assistant-b',0,'workflow_delivery',?1)",[proof.to_string()]).unwrap();
    mark_applied(c, id).unwrap();
}

#[test]
fn workflow_awareness_queries_are_strict_and_bounded() {
    let state: StateQuery = serde_json::from_value(json!({})).unwrap();
    assert_eq!(state.view, StateView::All);
    let inbox: MailboxQuery = serde_json::from_value(json!({})).unwrap();
    assert_eq!(inbox.limit, 20);
    assert_eq!(inbox.direction, MailboxDirection::Inbox);
    for invalid in [
        json!({"instanceId":"another"}),
        json!({"nodeId":"another"}),
        json!({"limit":0}),
        json!({"limit":51}),
        json!({"limit":-1}),
        json!({"direction":"all"}),
        json!({"cursor":-1}),
    ] {
        assert!(serde_json::from_value::<MailboxQuery>(invalid).is_err());
    }
    for invalid in [json!({"instanceId":"another"}), json!({"view":"messages"})] {
        assert!(serde_json::from_value::<StateQuery>(invalid).is_err());
    }
    assert!(MailboxQuery {
        limit: 51,
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(MailboxQuery {
        message_id: Some(" ".into()),
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(StateQuery {
        node_id: Some("x\ny".into()),
        ..Default::default()
    }
    .validate()
    .is_err());
}

#[test]
fn workflow_awareness_compact_bounds_preserve_current_node_and_detail_access() {
    let c = fixture();
    let identity = snapshot_for_run(&c, &conversation(&c, "a"), "a")
        .unwrap()
        .unwrap();
    let inputs: Vec<_> = (0..12).map(|i| format!("input-{i}")).collect();
    let missing: Vec<_> = (0..24).map(|i| format!("flow-{i}")).collect();
    let mut nodes:Vec<_>=(0..24).map(|i|json!({"nodeId":format!("extra-{i}"),"nodeName":"Node","kind":"agent","state":"idle","currentInputIds":inputs,"missingFlowIds":missing})).collect();
    nodes.push(json!({"nodeId":"b","nodeName":"Neighbor","kind":"agent","state":"idle","currentInputIds":inputs,"missingFlowIds":missing}));
    nodes.push(json!({"nodeId":"a","nodeName":"Current","kind":"agent","state":"idle","currentInputIds":inputs,"missingFlowIds":missing}));
    let compact = compact_nodes(&nodes, &identity);
    assert_eq!(compact.len(), 16);
    assert_eq!(compact[0]["nodeId"], "a");
    assert_eq!(compact[1]["nodeId"], "b");
    assert_eq!(compact[0]["currentInputIds"].as_array().unwrap().len(), 8);
    assert_eq!(compact[0]["currentInputCount"], 12);
    assert_eq!(compact[0]["missingFlowIds"].as_array().unwrap().len(), 16);
    assert_eq!(compact[0]["missingFlowCount"], 24);
    let full_task = "界".repeat(700);
    c.execute("UPDATE workflow_definitions SET definition_json=json_set(definition_json,'$.nodes[0].task',?1)",[&full_task]).unwrap();
    let overview = state_for_run(
        &c,
        &conversation(&c, "a"),
        "a",
        &StateQuery {
            view: StateView::Topology,
            ..Default::default()
        },
    )
    .unwrap();
    let first = &overview["topology"]["nodes"][0];
    assert_eq!(first["task"].as_str().unwrap().chars().count(), 512);
    assert_eq!(first["truncatedFields"], json!(["task"]));
    let detail = state_for_run(
        &c,
        &conversation(&c, "a"),
        "a",
        &StateQuery {
            view: StateView::Topology,
            node_id: Some("a".into()),
        },
    )
    .unwrap();
    assert_eq!(detail["topology"]["nodes"][0]["task"], full_task);
}

#[test]
fn workflow_awareness_topology_has_gates_roles_cycles_and_no_private_history() {
    let c = fixture();
    let view = state_for_run(&c, &conversation(&c, "a"), "a", &StateQuery::default()).unwrap();
    assert_eq!(view["background"], "Shared background");
    assert_eq!(view["topology"]["nodes"].as_array().unwrap().len(), 6);
    assert_eq!(view["topology"]["flows"].as_array().unwrap().len(), 8);
    let nodes = view["topology"]["nodes"].as_array().unwrap();
    assert_eq!(
        nodes
            .iter()
            .find(|node| node["nodeId"] == "batch-gate")
            .unwrap()["processingMode"],
        "batch"
    );
    assert_eq!(
        nodes.iter().find(|node| node["nodeId"] == "human").unwrap()["task"],
        "Accept release"
    );
    let a = nodes.iter().find(|node| node["nodeId"] == "a").unwrap();
    assert!(a["predecessors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["nodeId"] == "b"));
    let encoded = view.to_string();
    for forbidden in [
        "Private chat text",
        "\"x\":",
        "viewport",
        "permissionMode",
        "modelConfigId",
    ] {
        assert!(!encoded.contains(forbidden));
    }
    let focus = state_for_run(
        &c,
        &conversation(&c, "a"),
        "a",
        &StateQuery {
            view: StateView::Topology,
            node_id: Some("out".into()),
        },
    )
    .unwrap();
    assert_eq!(focus["topology"]["nodes"].as_array().unwrap().len(), 1);
    assert!(focus.get("runtime").is_none());
    assert!(state_for_run(
        &c,
        &conversation(&c, "a"),
        "a",
        &StateQuery {
            node_id: Some("unknown".into()),
            ..Default::default()
        }
    )
    .is_err());
}

#[test]
fn workflow_awareness_holds_back_inbox_until_proven_applied_without_side_effects() {
    let mut c = fixture();
    let left = send_message(&mut c, "left", "left", "Left secret before batch");
    let before = c.total_changes();
    let first = receiver(&c);
    assert_eq!(first["messages"][0]["deliveryStatus"], "collecting");
    assert_eq!(first["messages"][0]["bodyAvailable"], false);
    assert!(first["messages"][0].get("content").is_none());
    let awareness = awareness_for_run(&c, &conversation(&c, "a"), "a").unwrap();
    assert_eq!(awareness["recentSent"][0]["messageId"], left.messages[0].id);
    let b = awareness["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["nodeId"] == "b")
        .unwrap();
    assert_eq!(b["collectingMessageCount"], 1);
    assert_eq!(b["missingFlowIds"], json!(["right"]));
    let encoded = awareness.to_string();
    assert!(!encoded.contains("secret"));
    assert!(!encoded.contains("observedAt"));
    assert!(!encoded.contains("createdAt"));
    assert_eq!(before, c.total_changes());
    let right = send_message(&mut c, "right", "right", "Right secret");
    let id = &right.input_ids[0];
    assert_eq!(receiver(&c)["messages"][0]["deliveryStatus"], "pending");
    assert!(bind_input(&mut c, id, "b", "delivery-b").unwrap());
    assert_eq!(receiver(&c)["messages"][0]["bodyAvailable"], false);
    prove_delivery(&mut c, id);
    c.execute(
        "UPDATE conversation_turn_traces SET terminal_status='cancelled',completed_at=2 WHERE run_id='b'",
        [],
    )
    .unwrap();
    let before = c.total_changes();
    let delivered = receiver(&c);
    assert_eq!(delivered["messages"][0]["inputId"], *id);
    assert_eq!(delivered["messages"][0]["deliveryStatus"], "applied");
    assert_eq!(delivered["messages"][0]["runStatus"], "cancelled");
    assert_eq!(delivered["messages"][0]["content"], "Right secret");
    let awareness = awareness_for_run(&c, &conversation(&c, "a"), "a").unwrap();
    let b = awareness["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["nodeId"] == "b")
        .unwrap();
    assert_eq!(b["lastRunStatus"], "cancelled");
    assert_eq!(b["state"], "idle");
    assert_eq!(before, c.total_changes());
    let outbox = mailbox(
        &c,
        "a",
        MailboxQuery {
            direction: MailboxDirection::Outbox,
            ..Default::default()
        },
    );
    assert_eq!(outbox["messages"][0]["content"], "Right secret");
    assert_eq!(outbox["messages"][1]["content"], "Left secret before batch");
}

#[test]
fn workflow_awareness_inbox_failed_paused_and_unproved_statuses_never_bypass_delivery() {
    let mut c = fixture();
    send_message(&mut c, "left", "left", "Do not expose early");
    let sent = send_message(&mut c, "right", "right", "Do not expose early either");
    let id = &sent.input_ids[0];
    for status in [
        "pending",
        "claimed",
        "paused",
        "failed",
        "invalidated",
        "applied",
    ] {
        c.execute("UPDATE workflow_execution_inputs SET status=?1,input_json=json_set(input_json,'$.status',?1) WHERE input_id=?2",params![status,id]).unwrap();
        let view = receiver(&c);
        assert_eq!(view["messages"][0]["bodyAvailable"], false);
        assert!(!view.to_string().contains("Do not expose"));
    }
}

#[test]
fn workflow_awareness_mailbox_pages_filters_and_body_budget_are_explicit() {
    let mut c = fixture();
    let mut ids = Vec::new();
    for i in 0..5 {
        ids.push(
            send_message(&mut c, &format!("left-{i}"), "left", &format!("body-{i}")).messages[0]
                .id
                .clone(),
        );
    }
    let first = mailbox(
        &c,
        "a",
        MailboxQuery {
            direction: MailboxDirection::Outbox,
            limit: 2,
            ..Default::default()
        },
    );
    assert_eq!(first["messages"].as_array().unwrap().len(), 2);
    assert_eq!(first["messages"][0]["messageId"], ids[4]);
    let second = mailbox(
        &c,
        "a",
        MailboxQuery {
            direction: MailboxDirection::Outbox,
            limit: 2,
            cursor: first["nextCursor"].as_u64(),
            ..Default::default()
        },
    );
    assert_eq!(second["messages"][0]["messageId"], ids[2]);
    let third = mailbox(
        &c,
        "a",
        MailboxQuery {
            direction: MailboxDirection::Outbox,
            limit: 2,
            cursor: second["nextCursor"].as_u64(),
            ..Default::default()
        },
    );
    assert_eq!(third["messages"][0]["messageId"], ids[0]);
    assert!(third["nextCursor"].is_null());
    let exact = mailbox(
        &c,
        "a",
        MailboxQuery {
            direction: MailboxDirection::Outbox,
            message_id: Some(ids[1].clone()),
            ..Default::default()
        },
    );
    assert_eq!(exact["messages"].as_array().unwrap().len(), 1);
    let batch = send_message(&mut c, "right", "right", "paired");
    let exact_batch = mailbox(
        &c,
        "b",
        MailboxQuery {
            input_id: Some(batch.input_ids[0].clone()),
            ..Default::default()
        },
    );
    assert_eq!(exact_batch["messages"].as_array().unwrap().len(), 2);
    assert!(mailbox(
        &c,
        "b",
        MailboxQuery {
            message_id: Some("nonexistent-or-not-owned".into()),
            ..Default::default()
        }
    )["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    let large = "x".repeat(128_000);
    for i in 0..3 {
        send_message(&mut c, &format!("large-{i}"), "left", &large);
    }
    let bounded = mailbox(
        &c,
        "a",
        MailboxQuery {
            direction: MailboxDirection::Outbox,
            limit: 3,
            ..Default::default()
        },
    );
    assert_eq!(
        bounded["messages"][2]["withholdingReason"],
        "response_body_budget"
    );
    let detail = mailbox(
        &c,
        "a",
        MailboxQuery {
            direction: MailboxDirection::Outbox,
            message_id: bounded["messages"][2]["messageId"]
                .as_str()
                .map(String::from),
            ..Default::default()
        },
    );
    assert_eq!(
        detail["messages"][0]["content"].as_str().unwrap().len(),
        128_000
    );
}

#[test]
fn workflow_awareness_rejects_other_owners_stale_epochs_and_old_bindings() {
    let mut c = fixture();
    let sent = send_message(&mut c, "left", "left", "old private body");
    let a = conversation(&c, "a");
    let b = conversation(&c, "b");
    assert!(mailbox_for_run(&c, &a, "b", &MailboxQuery::default()).is_err());
    c.execute(
        "UPDATE workflow_execution_messages SET execution_version='old-epoch' WHERE message_id=?1",
        [&sent.messages[0].id],
    )
    .unwrap();
    assert!(receiver(&c)["messages"].as_array().unwrap().is_empty());
    assert!(mailbox(
        &c,
        "a",
        MailboxQuery {
            direction: MailboxDirection::Outbox,
            ..Default::default()
        }
    )["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    let another = send_message(&mut c, "new-left", "left", "wrong sender body");
    c.execute("UPDATE workflow_execution_messages SET message_json=json_set(message_json,'$.sourceConversationId','former-binding') WHERE message_id=?1",[&another.messages[0].id]).unwrap();
    assert!(mailbox(
        &c,
        "a",
        MailboxQuery {
            direction: MailboxDirection::Outbox,
            ..Default::default()
        }
    )["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    // Rebinding changes the execution version. The formerly admitted run cannot inspect it.
    c.execute(
        "UPDATE workflow_instance_bindings SET conversation_id=?1 WHERE node_id='b'",
        [&a],
    )
    .unwrap_err();
    c.execute("INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('replacement-b','Replacement',1,1)",[]).unwrap();
    c.execute(
        "UPDATE workflow_instance_bindings SET conversation_id='replacement-b' WHERE node_id='b'",
        [],
    )
    .unwrap();
    assert!(state_for_run(&c, &a, "a", &StateQuery::default()).is_err());
    assert!(mailbox_for_run(&c, &b, "b", &MailboxQuery::default()).is_err());
    add_run(&mut c, "replacement-b", "replacement-run");
    let replacement = mailbox_for_run(
        &c,
        "replacement-b",
        "replacement-run",
        &MailboxQuery::default(),
    )
    .unwrap();
    assert!(replacement["messages"].as_array().unwrap().is_empty());
    c.execute("UPDATE workflow_instances SET enabled=0", [])
        .unwrap();
    assert!(awareness_for_run(&c, &a, "a").is_err());
    assert_eq!(
        awareness_for_conversation(&c, &a).unwrap()["available"],
        false
    );
}
