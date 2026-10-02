use super::*;
use crate::storage::workflow_execution_repository::tests::{
    action, conversation, fixture, prove, send_mail, start_run,
};

#[test]
fn workflow_awareness_pending_preview_is_read_only_and_arrival_counts_are_monotonic() {
    let mut c = fixture(false);
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
    let mut c = fixture(false);
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
    let mut c = fixture(false);
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
    let c = fixture(false);
    let mut definition: Value = serde_json::from_str(
        &c.query_row(
            "SELECT definition_json FROM workflow_definitions WHERE workflow_id='template'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    definition["nodes"][1]["task"] = value!("x".repeat(2000));
    c.execute(
        "UPDATE workflow_definitions SET definition_json=?1",
        [definition.to_string()],
    )
    .unwrap();
    let chat = conversation(&c, "a");
    let snapshot = snapshot_for_conversation(&c, &chat).unwrap().unwrap();
    assert_eq!(snapshot.members[1].task.len(), 512);
    let all = state_for_run(&c, &chat, "run-a", &StateQuery::default()).unwrap();
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
        },
    )
    .unwrap();
    assert_eq!(focused["members"][0]["task"].as_str().unwrap().len(), 2000);
    assert!(focused.get("runtime").is_none());
}
#[test]
fn workflow_awareness_monitor_uses_mail_state_without_bodies_or_terminal_override() {
    let mut c = fixture(false);
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
    let mut c = fixture(false);
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
