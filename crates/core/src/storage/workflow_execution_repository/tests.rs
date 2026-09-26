use super::*;
use crate::storage::migrations::run_migrations;
use serde_json::json;
use std::collections::HashSet;

fn fixture(batch: bool, user: bool) -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    let agent = |id: &str| json!({"kind":"agent","id":id,"name":id,"x":0,"y":0,"permissionMode":"default","modelConfigId":"model","receives":"Receive artifacts","task":"Review quality","delivers":"Review report"});
    let target = if user {
        json!({"kind":"user","id":"b","name":"Human","x":0,"y":0,"task":"Inspect result manually"})
    } else {
        agent("b")
    };
    let flow = |id: &str, source: Option<&str>, target: &str| json!({"id":id,"name":id,"source":source.map(|v|json!({"kind":"node","nodeId":v})).unwrap_or(json!({"kind":"boundary"})),"target":{"kind":"node","nodeId":target}});
    let mut nodes = vec![agent("a"), target];
    let mut flows = vec![flow("entry", None, "a")];
    if batch {
        nodes.push(json!({"kind":"outputGate","id":"out","name":"out","x":0,"y":0,"selection":{"mode":"any","min":1,"max":2,"required":[],"groups":[]}}));
        nodes.push(json!({"kind":"inputGate","id":"in","name":"in","x":0,"y":0,"processingMode":"batch","busyPolicy":"inject"}));
        flows.extend([
            flow("out-bind", Some("a"), "out"),
            flow("left", Some("out"), "in"),
            flow("right", Some("out"), "in"),
            flow("in-bind", Some("in"), "b"),
        ]);
    } else {
        flows.push(flow("direct", Some("a"), "b"));
    }
    let definition = json!({"schemaVersion":1,"id":"template","name":"Template","description":"","background":"Shared project background","nodes":nodes,"flows":flows,"viewport":{"x":0,"y":0,"zoom":1},"boundaryPositions":{"input":{"x":0,"y":0}}});
    let mut bindings = vec![json!({"nodeId":"a","conversationId":null})];
    if !user {
        bindings.push(json!({"nodeId":"b","conversationId":null}));
    }
    let models = HashSet::from(["model".into()]);
    crate::storage::workflow_repository::request(
        &mut c,
        serde_json::from_value(
            json!({"operation":"save","definition":definition,"expectedRevision":0}),
        )
        .unwrap(),
        &models,
    )
    .unwrap();
    crate::storage::workflow_repository::request(&mut c,serde_json::from_value(json!({"operation":"saveInstance","id":"instance","templateId":"template","name":"Review workflow","color":"#123456","bindings":bindings,"expectedRevision":0,"expectedTemplateRevision":1})).unwrap(),&models).unwrap();
    let source = conversation(&c, "a");
    c.execute("INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES ('assistant',?1,'assistant','',1,0)",[&source]).unwrap();
    c.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES ('assistant',?1,'run',1,'in_progress',0,1,1)",[&source]).unwrap();
    bind_run(&mut c, &source, "run").unwrap();
    c
}
fn conversation(c: &Connection, node: &str) -> String {
    c.query_row("SELECT conversation_id FROM workflow_instance_bindings WHERE instance_id='instance' AND node_id=?1",[node],|r|r.get(0)).unwrap()
}
fn request(c: &Connection, call: &str, outputs: &[(&str, &str)]) -> SendRequest {
    let conversation_id = conversation(c, "a");
    let s = snapshot_for_conversation(c, &conversation_id)
        .unwrap()
        .unwrap();
    SendRequest {
        conversation_id,
        source_run_id: "run".into(),
        tool_call_id: call.into(),
        execution_version: s.execution_version,
        outputs: outputs
            .iter()
            .map(|(flow, message)| SendOutput {
                flow_id: (*flow).into(),
                message: (*message).into(),
            })
            .collect(),
    }
}
#[test]
fn workflow_execution_batch_accumulates_fifo_and_wraps_only_once() {
    let mut c = fixture(true, false);
    for (call, flow, text) in [("1", "left", "first-left"), ("2", "left", "second-left")] {
        let r = request(&c, call, &[(flow, text)]);
        assert!(send(&mut c, &r).unwrap().input_ids.is_empty());
    }
    let r = request(&c, "3", &[("right", "first-right")]);
    let receipt = send(&mut c, &r).unwrap();
    assert_eq!(receipt.input_ids.len(), 1);
    let inputs = pending_inputs(&c).unwrap();
    assert_eq!(inputs.len(), 1);
    let input = &inputs[0];
    assert_eq!(
        input
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>(),
        ["first-left", "first-right"]
    );
    assert_eq!(
        input
            .content
            .matches("[Shared workflow background]")
            .count(),
        1
    );
    assert_eq!(
        input
            .content
            .matches("[Workflow collaboration message]")
            .count(),
        1
    );
    assert!(input
        .content
        .contains("Sources: a (conversation: a), a (conversation: a)"));
    assert!(!input.content.contains("second-left"));
    assert_eq!(input.busy_policy, BusyPolicy::Inject);
    let r = request(&c, "4", &[("right", "second-right")]);
    assert_eq!(send(&mut c, &r).unwrap().input_ids.len(), 1);
    assert_eq!(pending_inputs(&c).unwrap().len(), 1); // FIFO admits only oldest input.
}
#[test]
fn workflow_execution_send_is_atomic_and_retry_is_idempotent() {
    let mut c = fixture(false, false);
    let r = request(&c, "1", &[("direct", "Review this")]);
    let first = send(&mut c, &r).unwrap();
    let second = send(&mut c, &r).unwrap();
    assert_eq!(first.id, second.id);
    assert!(second.duplicate);
    assert_eq!(
        runtime_snapshot(&c, "instance", None).unwrap().inputs.len(),
        1
    );
    let mut altered = r.clone();
    altered.outputs[0].message = "Other".into();
    assert!(send(&mut c, &altered).is_err());
    let bad = request(&c, "2", &[("direct", "valid"), ("missing", "invalid")]);
    assert!(send(&mut c, &bad).is_err());
    assert_eq!(
        runtime_snapshot(&c, "instance", None).unwrap().inputs.len(),
        1
    );
}
#[test]
fn workflow_execution_delivery_claim_stays_durable_across_disable_and_reenable() {
    let mut c = fixture(false, false);
    let r = request(&c, "1", &[("direct", "Review")]);
    let receipt = send(&mut c, &r).unwrap();
    let id = &receipt.input_ids[0];
    c.execute(
        "UPDATE workflow_instances SET enabled=0 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    assert!(pending_inputs(&c).unwrap().is_empty());
    assert!(!bind_input(&mut c, id, "receiver", "msg").unwrap());
    c.execute(
        "UPDATE workflow_instances SET enabled=1 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    assert!(bind_input(&mut c, id, "receiver", "msg").unwrap());
    assert!(pending_inputs(&c).unwrap().is_empty());
    assert!(!bind_input(&mut c, id, "different", "msg-2").unwrap());
    assert_eq!(bound_inputs(&c, "receiver").unwrap().len(), 1);
    assert!(mark_applied(&mut c, id).is_err()); // A claim alone is not a model receipt.
    let input = load_input(&c, id).unwrap().unwrap();
    let target = conversation(&c, "b");
    c.execute("INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES ('received',?1,'assistant','',1,0)",[&target]).unwrap();
    c.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES ('received',?1,'receiver',1,'in_progress',0,1,1)",[&target]).unwrap();
    let proof = json!({"type":"workflow_delivery","sequence":0,"inputId":id,"instanceId":input.instance_id,"workflowName":"Review workflow","content":input.content,"createdAt":input.created_at,"truncated":false});
    c.execute("INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json) VALUES ('received',0,'workflow_delivery',?1)",[proof.to_string()]).unwrap();
    mark_applied(&mut c, id).unwrap();
    mark_applied(&mut c, id).unwrap();
    assert_eq!(
        load_input(&c, id).unwrap().unwrap().status,
        InputStatus::Applied
    );
}
#[test]
fn workflow_execution_user_completion_has_no_downstream_send() {
    let mut c = fixture(false, true);
    let r = request(&c, "1", &[("direct", "Inspect this")]);
    let receipt = send(&mut c, &r).unwrap();
    let id = &receipt.input_ids[0];
    assert!(pending_inputs(&c).unwrap().is_empty());
    assert_eq!(
        load_input(&c, id).unwrap().unwrap().status,
        InputStatus::WaitingUser
    );
    complete_user_input(&mut c, id).unwrap();
    complete_user_input(&mut c, id).unwrap();
    let snapshot = runtime_snapshot(&c, "instance", None).unwrap();
    assert_eq!(snapshot.inputs.len(), 1);
    assert_eq!(snapshot.inputs[0].status, InputStatus::Completed);
    assert_eq!(
        snapshot.events.iter().filter(|e| e.kind == "sent").count(),
        1
    );
}
#[test]
fn workflow_execution_stale_graph_and_stopped_conversation_do_not_auto_run() {
    let mut c = fixture(false, false);
    let r = request(&c, "1", &[("direct", "Review")]);
    let receipt = send(&mut c, &r).unwrap();
    let chat = conversation(&c, "b");
    pause_conversation(&mut c, &chat).unwrap();
    assert!(pending_inputs(&c).unwrap().is_empty());
    assert_eq!(
        load_input(&c, &receipt.input_ids[0])
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Paused
    );
    resume_conversation(&mut c, &chat).unwrap();
    assert_eq!(pending_inputs(&c).unwrap().len(), 1);
    invalidate_instance(&c, "instance", "bindings changed").unwrap();
    assert!(pending_inputs(&c).unwrap().is_empty());
    assert_eq!(
        load_input(&c, &receipt.input_ids[0])
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Invalidated
    );
    c.execute(
        "UPDATE workflow_instances SET template_revision=2 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    let mut stale = r;
    stale.tool_call_id = "2".into();
    assert!(send(&mut c, &stale).is_err());
}
#[test]
fn workflow_execution_run_without_identity_cannot_gain_it_mid_turn() {
    let mut c = fixture(false, false);
    let chat = conversation(&c, "a");
    c.execute(
        "UPDATE workflow_instances SET enabled=0 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    assert!(bind_run(&mut c, &chat, "no-workflow").unwrap().is_none());
    c.execute(
        "UPDATE workflow_instances SET enabled=1 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    assert!(bind_run(&mut c, &chat, "no-workflow").unwrap().is_none());
    assert!(bind_run(&mut c, &chat, "run").unwrap().is_some());
}
#[test]
fn workflow_execution_output_gate_rules_cover_all_presets_and_custom_groups() {
    let available = vec!["a".into(), "b".into(), "c".into()];
    for (mode, min, max, chosen, expected) in [
        (crate::workflow::Mode::All, 0, 0, vec!["a"], false),
        (crate::workflow::Mode::All, 0, 0, vec!["a", "b", "c"], true),
        (crate::workflow::Mode::One, 0, 0, vec!["a", "b"], false),
        (crate::workflow::Mode::Any, 0, 0, vec!["a", "c"], true),
        (crate::workflow::Mode::Exact, 2, 0, vec!["b", "c"], true),
        (
            crate::workflow::Mode::Range,
            1,
            2,
            vec!["a", "b", "c"],
            false,
        ),
    ] {
        let rule = Rule {
            mode,
            min,
            max,
            required: vec![],
            groups: vec![],
        };
        assert_eq!(
            validate_selection(
                Some(&rule),
                &available,
                &chosen.into_iter().map(str::to_owned).collect::<Vec<_>>()
            )
            .is_ok(),
            expected
        );
    }
    let rule = Rule {
        mode: crate::workflow::Mode::Custom,
        min: 0,
        max: 0,
        required: vec!["a".into()],
        groups: vec![crate::workflow::Group {
            id: "choice".into(),
            flow_ids: vec!["b".into(), "c".into()],
            min: 1,
            max: 1,
        }],
    };
    assert!(validate_selection(Some(&rule), &available, &["a".into(), "c".into()]).is_ok());
    assert!(validate_selection(Some(&rule), &available, &available).is_err());
}

#[test]
fn workflow_execution_stop_blocks_new_sends_but_preserves_successful_retry_receipt() {
    let mut c = fixture(false, false);
    let accepted = request(&c, "before-stop", &[("direct", "Accepted")]);
    send(&mut c, &accepted).unwrap();
    let rejected = request(&c, "after-stop", &[("direct", "Do not forward")]);
    let source = conversation(&c, "a");
    pause_conversation(&mut c, &source).unwrap();
    assert!(send(&mut c, &rejected).unwrap_err().contains("stopped"));
    assert!(send(&mut c, &accepted).unwrap().duplicate);
}
#[test]
fn workflow_execution_monitor_is_lightweight_and_keeps_pending_users_after_history() {
    let mut c = fixture(false, true);
    let first = request(&c, "old-wait", &[("direct", "Private incoming body")]);
    let oldest = send(&mut c, &first).unwrap().input_ids[0].clone();
    for index in 0..140 {
        let r = request(&c, &format!("later-{index}"), &[("direct", "Later task")]);
        let id = send(&mut c, &r).unwrap().input_ids[0].clone();
        complete_user_input(&mut c, &id).unwrap();
    }
    let view = runtime_snapshot(&c, "instance", None).unwrap();
    assert!(view
        .inputs
        .iter()
        .any(|i| i.id == oldest && i.status == InputStatus::WaitingUser));
    assert_eq!(view.inputs[0].id, oldest);
    assert!(view
        .inputs
        .iter()
        .all(|i| i.content.is_empty() && i.messages.iter().all(|m| m.content.is_empty())));
    assert!(load_input(&c, &oldest)
        .unwrap()
        .unwrap()
        .content
        .contains("Private incoming body"));
}

#[test]
fn workflow_execution_snapshot_and_envelope_explain_downstream_intake() {
    for (batch, user) in [(false, false), (true, false), (false, true), (true, true)] {
        let c = fixture(batch, user);
        let source = conversation(&c, "a");
        let snapshot = snapshot_for_conversation(&c, &source).unwrap().unwrap();
        let rule = &snapshot.outputs[0].input_rule;
        if batch {
            assert!(rule.contains("waits for every incoming flow"));
        } else {
            assert!(rule.contains("Each incoming message"));
        }
        if user {
            assert!(rule.contains("user to confirm completion"));
            assert!(rule.contains("never sends a downstream message automatically"));
        } else if batch {
            assert!(rule.contains("next safe input boundary"));
        } else {
            assert!(rule.contains("FIFO order until the current task finishes"));
        }
        let body = assemble_message(&snapshot, &[]);
        assert!(body.contains(rule));
        let wire = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            wire["outputs"][0]["inputRule"].as_str(),
            Some(rule.as_str())
        );
    }
}

#[test]
fn workflow_execution_discard_is_explicit_idempotent_and_releases_fifo_without_replay() {
    let mut c = fixture(false, false);
    let first = request(&c, "first", &[("direct", "Ambiguous delivery")]);
    let first = send(&mut c, &first).unwrap().input_ids[0].clone();
    let second = request(&c, "second", &[("direct", "Next delivery")]);
    let second = send(&mut c, &second).unwrap().input_ids[0].clone();
    assert!(discard_failed(&mut c, &first).is_err());
    assert!(bind_input(&mut c, &first, "lost-run", "lost-message").unwrap());
    fail_input(&mut c, &first, "Delivery outcome unknown").unwrap();
    assert!(pending_inputs(&c).unwrap().is_empty());
    c.execute(
        "UPDATE workflow_instances SET enabled=0 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    discard_failed(&mut c, &first).unwrap();
    discard_failed(&mut c, &first).unwrap();
    let skipped = load_input(&c, &first).unwrap().unwrap();
    assert_eq!(skipped.status, InputStatus::Invalidated);
    assert_eq!(skipped.error.as_deref(), Some("Delivery outcome unknown"));
    assert!(pending_inputs(&c).unwrap().is_empty());
    c.execute(
        "UPDATE workflow_instances SET enabled=1 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    assert_eq!(
        pending_inputs(&c)
            .unwrap()
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        [second.as_str()]
    );
    let events = runtime_snapshot(&c, "instance", None).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "discarded" && e.input_id.as_deref() == Some(&first))
            .count(),
        1
    );
    // Log retention never changes the operation's idempotent result.
    c.execute(
        "DELETE FROM workflow_execution_events WHERE kind='discarded'",
        [],
    )
    .unwrap();
    discard_failed(&mut c, &first).unwrap();
    assert_eq!(
        runtime_snapshot(&c, "instance", None)
            .unwrap()
            .events
            .iter()
            .filter(|e| e.kind == "discarded")
            .count(),
        0
    );
    assert!(discard_failed(&mut c, &second).is_err());
}

#[test]
fn workflow_execution_completion_receipt_updates_unread_once_and_keeps_old_referenced_inputs() {
    let mut c = fixture(false, false);
    let first = request(&c, "applied", &[("direct", "Long-running task")]);
    let first = send(&mut c, &first).unwrap().input_ids[0].clone();
    assert!(bind_input(&mut c, &first, "long-run", "long-message").unwrap());
    let mut applied = load_input(&c, &first).unwrap().unwrap();
    applied.status = InputStatus::Applied;
    c.execute(
        "UPDATE workflow_execution_inputs SET input_json=?1,status='applied' WHERE input_id=?2",
        params![json(&applied).unwrap(), applied.id],
    )
    .unwrap();
    let target = conversation(&c, "b");
    let failed = request(&c, "failed", &[("direct", "Do not hide this diagnostic")]);
    let failed = send(&mut c, &failed).unwrap().input_ids[0].clone();
    fail_input(&mut c, &failed, "Ambiguous dispatch outcome").unwrap();
    // More than the historical tail and event page arrive while the first input is running.
    for index in 0..270 {
        let r = request(
            &c,
            &format!("later-{index}"),
            &[("direct", "Later completed history")],
        );
        let id = send(&mut c, &r).unwrap().input_ids[0].clone();
        let mut input = load_input(&c, &id).unwrap().unwrap();
        input.status = InputStatus::Invalidated;
        c.execute("UPDATE workflow_execution_inputs SET input_json=?1,status='invalidated' WHERE input_id=?2",params![json(&input).unwrap(),input.id]).unwrap();
        event(&c, "instance", None, &[], "completed").unwrap();
    }
    let before = runtime_snapshot(&c, "instance", None).unwrap();
    assert!(!before.inputs.iter().any(|i| i.id == first));
    assert!(before
        .inputs
        .iter()
        .any(|i| i.id == failed && i.status == InputStatus::Failed));
    c.execute(
        "UPDATE conversations SET unread_at=NULL WHERE id=?1",
        [&target],
    )
    .unwrap();
    mark_run_unread(&mut c, "long-run").unwrap();
    let view = runtime_snapshot(&c, "instance", Some(before.sequence)).unwrap();
    assert_eq!(view.events.len(), 1);
    assert_eq!(view.events[0].kind, "run_completed");
    assert_eq!(view.events[0].input_id.as_deref(), Some(first.as_str()));
    assert!(view.inputs.iter().any(|i| i.id == first
        && i.content.is_empty()
        && i.conversation_id.as_deref() == Some(&target)));
    let unread: Option<i64> = c
        .query_row(
            "SELECT unread_at FROM conversations WHERE id=?1",
            [&target],
            |r| r.get(0),
        )
        .unwrap();
    assert!(unread.is_some());
    // After the user reads it, a repeated terminal callback must not mark it unread again.
    c.execute(
        "UPDATE conversations SET unread_at=NULL WHERE id=?1",
        [&target],
    )
    .unwrap();
    c.execute(
        "DELETE FROM workflow_execution_events WHERE kind='run_completed'",
        [],
    )
    .unwrap();
    mark_run_unread(&mut c, "long-run").unwrap();
    let unread: Option<i64> = c
        .query_row(
            "SELECT unread_at FROM conversations WHERE id=?1",
            [&target],
            |r| r.get(0),
        )
        .unwrap();
    assert!(unread.is_none());
    assert!(!runtime_snapshot(&c, "instance", None)
        .unwrap()
        .events
        .iter()
        .any(|e| e.kind == "run_completed"));
}
