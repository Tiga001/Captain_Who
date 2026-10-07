use super::*;
use crate::storage::workflow_execution_repository::tests::{
    conversation, fixture, send_mail, start_run,
};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

#[test]
fn runtime_summary_matches_counts_and_pauses_without_reading_envelope_bodies() {
    let mut c = fixture();
    let receipt = send_mail(
        &mut c,
        "summary",
        &[("b", "large body"), ("b", "pending"), ("c", "other")],
    );
    start_run(&mut c, "b", "run-b");
    assert!(bind_input(&mut c, &receipt.input_ids[0], "run-b", "delivery-b").unwrap());
    let b = conversation(&c, "b");
    pause_conversation(&mut c, &b).unwrap();
    let full = runtime_snapshot(&c, "instance", None).unwrap();
    let mut counts = BTreeMap::<String, u64>::new();
    for input in &full.inputs {
        if input.mail_status == MailStatus::Pending {
            *counts.entry(input.node_id.clone()).or_default() += input.messages.len() as u64;
        }
    }
    c.execute(
        "UPDATE workflow_mail_inputs SET input_json=json_object('invalid',hex(zeroblob(1048576)))",
        [],
    )
    .unwrap();
    c.execute("UPDATE workflow_mail_messages SET message_json=json_object('invalid',hex(zeroblob(1048576)))", []).unwrap();
    c.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Read {
            table_name: "workflow_mail_inputs",
            ..
        }
        | AuthAction::Read {
            table_name: "workflow_mail_messages",
            column_name: "message_json",
            ..
        }
        | AuthAction::Read {
            table_name: "conversation_turn_trace_items",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    let snapshot = runtime_summary(&c, "instance", None).unwrap();
    c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert!(snapshot.inputs.is_empty() && snapshot.input_runs.is_empty());
    assert_eq!(snapshot.sequence, full.sequence);
    assert_eq!(
        snapshot.paused_conversation_ids,
        full.paused_conversation_ids
    );
    assert_eq!(
        serde_json::to_value(&snapshot.events).unwrap(),
        serde_json::to_value(full.events).unwrap()
    );
    let summary = snapshot.summary.unwrap();
    assert_eq!(
        summary
            .pending_by_node
            .into_iter()
            .map(|row| (row.node_id, row.count))
            .collect::<BTreeMap<_, _>>(),
        counts
    );
    assert_eq!(summary.conversation_changes.len(), 1);
    assert_eq!(summary.conversation_changes[0].conversation_id, b);
    assert!(summary.conversation_changes[0].sequence <= snapshot.sequence);
    assert!(
        runtime_snapshot(&c, "instance", None).is_err(),
        "the explicit full read remains strict"
    );
}

#[test]
fn runtime_summary_recovers_retired_receipts_structure_and_preferences_beyond_event_window() {
    let mut c = fixture();
    let receipt = send_mail(&mut c, "old", &[("b", "delivered before removal")]);
    let b = conversation(&c, "b");
    start_run(&mut c, "b", "run-b");
    assert!(bind_input(&mut c, &receipt.input_ids[0], "run-b", "old-delivery").unwrap());
    c.execute_batch(
        "INSERT INTO workflow_mail_events(instance_id,source_node_id,target_node_id,kind,created_at)
         VALUES ('instance','a','c','member_model_changed',1),
                ('instance','a','c','member_permissions_changed',1),
                ('instance','a','b','member_model_changed',1),
                ('instance','a',NULL,'members_changed',1);"
    ).unwrap();
    let structure: u64 = c.last_insert_rowid() as u64;
    c.execute(
        "DELETE FROM workflow_instance_bindings WHERE instance_id='instance' AND node_id='b'",
        [],
    )
    .unwrap();
    c.execute("INSERT INTO workflow_mail_events(instance_id,input_id,kind,created_at) VALUES ('instance',?1,'run_completed',2)", [&receipt.input_ids[0]]).unwrap();
    let terminal: u64 = c.last_insert_rowid() as u64;
    c.execute_batch(
        "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<600)
         INSERT INTO workflow_mail_events(instance_id,kind,created_at)
         SELECT 'instance','queued',x+2 FROM n;",
    )
    .unwrap();
    let snapshot = runtime_summary(&c, "instance", None).unwrap();
    let summary = snapshot.summary.clone().unwrap();
    assert_eq!(summary.structure_revision, structure);
    assert_eq!(
        summary.conversation_changes,
        vec![ConversationMailChange {
            conversation_id: b.clone(),
            sequence: terminal
        }]
    );
    assert_eq!(snapshot.events.len(), 514);
    assert_eq!(
        snapshot
            .events
            .iter()
            .filter(|event| event.target_node_id.as_deref() == Some("c"))
            .count(),
        2
    );
    assert!(!snapshot
        .events
        .iter()
        .any(|event| event.target_node_id.as_deref() == Some("b")));
    assert!(snapshot
        .events
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    assert!(snapshot.preference_updates.is_empty());
    let caught_up = runtime_summary(&c, "instance", Some(snapshot.sequence)).unwrap();
    assert_eq!(
        caught_up.summary, snapshot.summary,
        "summary facts never depend on the animation cursor"
    );
    assert_eq!(
        caught_up.events.len(),
        2,
        "historical preference invalidations remain recoverable"
    );
    c.execute("INSERT INTO workflow_mail_events(instance_id,input_id,kind,created_at) VALUES ('instance',?1,'run_completed',999)", [&receipt.input_ids[0]]).unwrap();
    let late = runtime_summary(&c, "instance", Some(snapshot.sequence)).unwrap();
    assert_eq!(
        late.summary.unwrap().conversation_changes,
        vec![ConversationMailChange {
            conversation_id: b,
            sequence: late.sequence
        }]
    );
    assert!(runtime_summary(&c, "missing", None).is_err());
}

#[test]
fn runtime_summary_contract_fixture_is_produced_by_storage_and_round_trips() {
    let c = fixture();
    c.execute_batch(
        "INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('fixture-b','Fixture member',1,1);
         UPDATE workflow_instance_bindings SET conversation_id='fixture-b' WHERE instance_id='instance' AND node_id='b';
         INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,run_id,delivery_id,created_at,updated_at)
         VALUES ('delivered-input','instance','epoch','b','fixture-b','{}','completed','run-b','delivery-b',1,1),
                ('pending-input','instance','epoch','b','fixture-b','{}','pending',NULL,NULL,1,1);
         INSERT INTO workflow_mail_messages(message_id,instance_id,execution_version,node_id,recipient_conversation_id,mail_status,message_json,input_id,created_at)
         VALUES ('pending-mail','instance','epoch','b','fixture-b','pending','{}','pending-input',1);
         INSERT INTO workflow_mail_pauses(conversation_id,created_at) VALUES ('fixture-b',1);
         INSERT INTO workflow_mail_events(sequence,instance_id,input_id,message_id,source_node_id,target_node_id,kind,created_at)
         VALUES (1,'instance','pending-input','pending-mail','a','b','queued',1),
                (2,'instance','delivered-input','delivered-mail','a','b','accepted',2),
                (3,'instance','delivered-input','delivered-mail','a','b','delivered',3),
                (4,'instance',NULL,NULL,'a',NULL,'members_changed',4),
                (5,'instance',NULL,NULL,'a','b','member_model_changed',5),
                (6,'instance','delivered-input',NULL,NULL,NULL,'run_completed',6);"
    ).unwrap();
    let snapshot = runtime_summary(&c, "instance", None).unwrap();
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packages/protocol/src/fixtures/workflowRuntimeSummary.json"
    )))
    .unwrap();
    assert_eq!(serde_json::to_value(&snapshot).unwrap(), fixture);
    let decoded: RuntimeSnapshot = serde_json::from_value(fixture.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), fixture);
}

#[test]
fn runtime_summary_does_not_truncate_historical_conversations_or_cross_instances() {
    let c = fixture();
    c.execute_batch(
        "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<5000)
         INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,
             conversation_id,input_json,status,delivery_id,created_at,updated_at)
         SELECT 'old-input-'||x,'instance','old-epoch','retired','old-chat-'||x,
             '{}','completed','old-delivery-'||x,1,1 FROM n;
         INSERT INTO workflow_mail_events(instance_id,input_id,kind,created_at)
         SELECT 'instance',input_id,'run_completed',1 FROM workflow_mail_inputs;
         INSERT INTO workflow_mail_events(instance_id,input_id,kind,created_at)
         VALUES ('other-instance','old-input-1','run_completed',2);",
    )
    .unwrap();
    let summary = runtime_summary(&c, "instance", Some(u64::MAX)).unwrap();
    assert_eq!(summary.sequence, 5000);
    assert!(summary.events.is_empty());
    let changes = summary.summary.unwrap().conversation_changes;
    assert_eq!(changes.len(), 5000);
    assert!(changes
        .iter()
        .all(|change| change.sequence <= summary.sequence));
}
