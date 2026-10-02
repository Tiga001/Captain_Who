use super::*;
use crate::storage::migrations::run_migrations;
use crate::workflow_awareness::MailboxQuery;
use serde_json::json;
use std::collections::HashSet;

pub(super) fn fixture(user: bool) -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    let agent = |id: &str| json!({"kind":"agent","id":id,"name":id,"x":0,"y":0,"permissionMode":"default","modelConfigId":"model","receives":"Artifacts","task":"Review quality","delivers":"Review report"});
    let mut nodes = vec![agent("a"), agent("b"), agent("c")];
    if user {
        nodes.push(
            json!({"kind":"user","id":"human","name":"Human","x":0,"y":0,"task":"Inspect results"}),
        );
    }
    let definition = json!({"schemaVersion":1,"id":"template","name":"Template","description":"Team","background":"Shared background","nodes":nodes,"viewport":{"x":0,"y":0,"zoom":1}});
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
    crate::storage::workflow_repository::request(&mut c,serde_json::from_value(json!({"operation":"saveInstance","id":"instance","templateId":"template","name":"Review workflow","color":"#123456","bindings":[{"nodeId":"a","conversationId":null},{"nodeId":"b","conversationId":null},{"nodeId":"c","conversationId":null}],"expectedRevision":0,"expectedTemplateRevision":1})).unwrap(),&models).unwrap();
    start_run(&mut c, "a", "run-a");
    c
}
pub(super) fn conversation(c: &Connection, node: &str) -> String {
    c.query_row("SELECT conversation_id FROM workflow_instance_bindings WHERE instance_id='instance' AND node_id=?1",[node],|r|r.get(0)).unwrap()
}
pub(super) fn start_run(c: &mut Connection, node: &str, run: &str) {
    let chat = conversation(c, node);
    c.execute("INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES(?1,?2,'assistant','',1,(SELECT COALESCE(MAX(position),-1)+1 FROM messages WHERE conversation_id=?2))",params![run,chat]).unwrap();
    c.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES(?1,?2,?1,?3,'in_progress',0,1,1)",params![run,chat,crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION]).unwrap();
    bind_run(c, &chat, run).unwrap();
}
pub(super) fn request(c: &Connection, call: &str, messages: &[(&str, &str)]) -> SendRequest {
    let chat = conversation(c, "a");
    let identity = snapshot_for_run(c, &chat, "run-a").unwrap().unwrap();
    SendRequest {
        conversation_id: chat,
        source_run_id: "run-a".into(),
        tool_call_id: call.into(),
        execution_version: identity.execution_version,
        messages: messages
            .iter()
            .map(|(target, message)| SendOutput {
                target_node_id: (*target).into(),
                message: (*message).into(),
                reply_to_message_id: None,
            })
            .collect(),
    }
}
pub(super) fn action(
    c: &Connection,
    node: &str,
    run: &str,
    call: &str,
    action: MutationAction,
    messages: &[String],
) -> MutationRequest {
    let chat = conversation(c, node);
    let identity = snapshot_for_run(c, &chat, run).unwrap().unwrap();
    MutationRequest {
        conversation_id: chat,
        source_run_id: run.into(),
        tool_call_id: call.into(),
        execution_version: identity.execution_version,
        action,
        message_ids: messages.to_vec(),
    }
}
pub(super) fn prove(c: &Connection, input_id: &str) {
    let input = load_input(c, input_id).unwrap().unwrap();
    let sequence:u64=c.query_row("SELECT COALESCE(MAX(sequence),-1)+1 FROM conversation_turn_trace_items WHERE assistant_message_id=?1",[&input.run_id],|r|r.get(0)).unwrap();
    let proof = json!({"type":"workflow_delivery","sequence":sequence,"inputId":input_id,"instanceId":input.instance_id,"workflowName":"Review workflow","content":input.content,"createdAt":input.created_at,"truncated":false});
    c.execute("INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json) VALUES(?1,?2,'workflow_delivery',?3)",params![input.run_id,sequence,proof.to_string()]).unwrap();
}

pub(super) fn send_mail(c: &mut Connection, call: &str, messages: &[(&str, &str)]) -> SendReceipt {
    let req = request(c, call, messages);
    send(c, &req).unwrap()
}
fn status(c: &Connection, receipt: &SendReceipt, index: usize) -> MailStatus {
    load_input(c, &receipt.input_ids[index])
        .unwrap()
        .unwrap()
        .mail_status
}

#[test]
fn workflow_execution_any_member_sends_atomically_and_retry_is_idempotent() {
    let mut c = fixture(false);
    let req = request(&c, "send", &[("b", "first"), ("c", "second")]);
    let receipt = send(&mut c, &req).unwrap();
    let retry = send(&mut c, &req).unwrap();
    assert_eq!(receipt.id, retry.id);
    assert!(retry.duplicate);
    assert_eq!(receipt.input_ids.len(), 2);
    assert_eq!(receipt.messages[0].target_node_name, "b");
    assert_eq!(
        load_input(&c, &receipt.input_ids[0])
            .unwrap()
            .unwrap()
            .messages
            .len(),
        1
    );
    let mut changed = req.clone();
    changed.messages[0].message = "different".into();
    assert!(send(&mut c, &changed).is_err());
    let invalid = request(&c, "bad", &[("b", "valid"), ("missing", "invalid")]);
    assert!(send(&mut c, &invalid).is_err());
    assert_eq!(
        runtime_snapshot(&c, "instance", None).unwrap().inputs.len(),
        2
    );
}
#[test]
fn workflow_execution_idle_fifo_and_explicit_accept_are_separate() {
    let mut c = fixture(false);
    let receipt = send_mail(
        &mut c,
        "send",
        &[("b", "oldest"), ("b", "later"), ("b", "last")],
    );
    start_run(&mut c, "b", "run-b");
    assert_eq!(
        pending_inputs(&c)
            .unwrap()
            .iter()
            .map(|i| &i.id)
            .collect::<Vec<_>>(),
        vec![&receipt.input_ids[0]]
    );
    assert!(!bind_input(&mut c, &receipt.input_ids[1], "run-b", "auto-later").unwrap());
    let req = action(
        &c,
        "b",
        "run-b",
        "accept-later",
        MutationAction::Accept,
        &[receipt.messages[1].id.clone()],
    );
    let result = mutate(&mut c, &req).unwrap();
    assert_eq!(result["messages"][0]["success"], true);
    assert_eq!(result["messages"][0]["content"], "later");
    assert_eq!(mutate(&mut c, &req).unwrap(), result);
    assert_eq!(status(&c, &receipt, 1), MailStatus::Processing);
    assert_eq!(bound_inputs(&c, "run-b").unwrap().len(), 1);
    assert_eq!(status(&c, &receipt, 0), MailStatus::Pending);
    assert!(bind_input(&mut c, &receipt.input_ids[0], "run-b", "auto-oldest").unwrap());
    assert_eq!(bound_inputs(&c, "run-b").unwrap().len(), 2);
}
#[test]
fn workflow_execution_completion_requires_durable_context_and_never_regresses() {
    let mut c = fixture(false);
    let receipt = send_mail(&mut c, "send", &[("b", "work")]);
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
    let incomplete = action(
        &c,
        "b",
        "run-b",
        "complete-too-soon",
        MutationAction::Complete,
        &[receipt.messages[0].id.clone()],
    );
    assert_eq!(
        mutate(&mut c, &incomplete).unwrap()["messages"][0]["success"],
        false
    );
    assert!(mark_applied(&mut c, &receipt.input_ids[0]).is_err());
    prove(&c, &receipt.input_ids[0]);
    let complete = action(
        &c,
        "b",
        "run-b",
        "complete",
        MutationAction::Complete,
        &[receipt.messages[0].id.clone()],
    );
    assert_eq!(
        mutate(&mut c, &complete).unwrap()["messages"][0]["status"],
        "processed"
    );
    mark_applied(&mut c, &receipt.input_ids[0]).unwrap();
    settle_run(&c, "run-b", "failed").unwrap();
    settle_run(&c, "run-b", "cancelled").unwrap();
    fail_input(&mut c, &receipt.input_ids[0], "late error").unwrap();
    recover_claims(&mut c).unwrap();
    assert_eq!(status(&c, &receipt, 0), MailStatus::Processed);
    assert_eq!(
        load_input(&c, &receipt.input_ids[0])
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Completed
    );
}
#[test]
fn workflow_execution_stop_pauses_pending_without_turning_it_into_stopped_mail() {
    let mut c = fixture(false);
    let receipt = send_mail(&mut c, "send", &[("b", "current"), ("b", "future")]);
    start_run(&mut c, "b", "run-b");
    bind_input(&mut c, &receipt.input_ids[0], "run-b", "initial").unwrap();
    prove(&c, &receipt.input_ids[0]);
    let chat = conversation(&c, "b");
    pause_conversation(&mut c, &chat).unwrap();
    settle_run(&c, "run-b", "cancelled").unwrap();
    assert_eq!(status(&c, &receipt, 0), MailStatus::Stopped);
    assert_eq!(status(&c, &receipt, 1), MailStatus::Pending);
    assert!(pending_inputs(&c).unwrap().is_empty());
    resume_conversation(&mut c, &chat).unwrap();
    assert_eq!(pending_inputs(&c).unwrap()[0].id, receipt.input_ids[1]);
}
#[test]
fn workflow_execution_terminal_failure_releases_fifo_and_normal_completion_is_durable() {
    let mut c = fixture(false);
    let receipt = send_mail(&mut c, "send", &[("b", "fails"), ("b", "next")]);
    start_run(&mut c, "b", "run-b");
    bind_input(&mut c, &receipt.input_ids[0], "run-b", "first").unwrap();
    settle_run(&c, "run-b", "failed").unwrap();
    assert_eq!(status(&c, &receipt, 0), MailStatus::Failed);
    assert_eq!(pending_inputs(&c).unwrap()[0].id, receipt.input_ids[1]);
    c.execute(
        "UPDATE conversation_turn_traces SET terminal_status='failed',completed_at=2 WHERE run_id='run-b'",
        [],
    )
    .unwrap();
    start_run(&mut c, "b", "run-next");
    bind_input(&mut c, &receipt.input_ids[1], "run-next", "next").unwrap();
    prove(&c, &receipt.input_ids[1]);
    settle_run(&c, "run-next", "completed").unwrap();
    assert_eq!(status(&c, &receipt, 1), MailStatus::Processed);
}
#[test]
fn workflow_execution_recall_is_sender_only_and_cannot_win_after_accept() {
    let mut c = fixture(false);
    let receipt = send_mail(&mut c, "send", &[("b", "retract"), ("b", "already taken")]);
    start_run(&mut c, "b", "run-b");
    start_run(&mut c, "c", "run-c");
    let wrong = action(
        &c,
        "c",
        "run-c",
        "not-owner",
        MutationAction::Recall,
        &[receipt.messages[0].id.clone()],
    );
    let result = mutate(&mut c, &wrong).unwrap();
    assert_eq!(result["messages"][0]["success"], false);
    assert!(result["messages"][0].get("content").is_none());
    let recall = action(
        &c,
        "a",
        "run-a",
        "recall",
        MutationAction::Recall,
        &[receipt.messages[0].id.clone()],
    );
    let recalled = mutate(&mut c, &recall).unwrap();
    assert_eq!(recalled["messages"][0]["status"], "recalled");
    assert_eq!(mutate(&mut c, &recall).unwrap(), recalled);
    let accept = action(
        &c,
        "b",
        "run-b",
        "accept",
        MutationAction::Accept,
        &[receipt.messages[1].id.clone()],
    );
    mutate(&mut c, &accept).unwrap();
    let late = action(
        &c,
        "a",
        "run-a",
        "too-late",
        MutationAction::Recall,
        &[receipt.messages[1].id.clone()],
    );
    assert_eq!(
        mutate(&mut c, &late).unwrap()["messages"][0]["success"],
        false
    );
    let events = runtime_snapshot(&c, "instance", None).unwrap().events;
    let event = events.iter().find(|e| e.kind == "recalled").unwrap();
    assert_eq!(event.source_node_id.as_deref(), Some("b"));
    assert_eq!(event.target_node_id.as_deref(), Some("a"));
}
#[test]
fn workflow_execution_recovery_does_not_complete_unproven_claims() {
    let mut c = fixture(false);
    let receipt = send_mail(&mut c, "send", &[("b", "not received")]);
    start_run(&mut c, "b", "run-b");
    bind_input(&mut c, &receipt.input_ids[0], "run-b", "initial").unwrap();
    c.execute(
        "UPDATE conversation_turn_traces SET terminal_status='completed',completed_at=2 WHERE run_id='run-b'",
        [],
    )
    .unwrap();
    recover_claims(&mut c).unwrap();
    assert_eq!(status(&c, &receipt, 0), MailStatus::Failed);
}
#[test]
fn workflow_execution_user_completion_is_idempotent_and_does_not_send() {
    let mut c = fixture(true);
    let receipt = send_mail(&mut c, "send", &[("human", "Review this")]);
    assert!(pending_inputs(&c).unwrap().is_empty());
    complete_user_input(&mut c, &receipt.input_ids[0]).unwrap();
    complete_user_input(&mut c, &receipt.input_ids[0]).unwrap();
    assert_eq!(status(&c, &receipt, 0), MailStatus::Processed);
    assert_eq!(
        runtime_snapshot(&c, "instance", None).unwrap().inputs.len(),
        1
    );
}
#[test]
fn workflow_execution_epoch_changes_keep_mail_but_rebinding_fences_original_recipient() {
    let mut c = fixture(false);
    let receipt = send_mail(&mut c, "send", &[("b", "keep me")]);
    c.execute("UPDATE workflow_instances SET template_revision=template_revision+1 WHERE instance_id='instance'",[]).unwrap();
    c.execute(
        "UPDATE workflow_definitions SET revision=revision+1 WHERE workflow_id='template'",
        [],
    )
    .unwrap();
    assert_eq!(pending_inputs(&c).unwrap()[0].id, receipt.input_ids[0]);
    start_run(&mut c, "b", "run-b");
    let b = conversation(&c, "b");
    let inbox = mailbox_for_run(&c, &b, "run-b", &MailboxQuery::default()).unwrap();
    assert_eq!(inbox["messages"][0]["content"], "keep me");
    let replacement = conversation(&c, "c");
    c.execute(
        "DELETE FROM workflow_instance_bindings WHERE node_id='c'",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE workflow_instance_bindings SET conversation_id=?1 WHERE node_id='b'",
        [&replacement],
    )
    .unwrap();
    reconcile_recipients(&c, "instance").unwrap();
    assert_eq!(status(&c, &receipt, 0), MailStatus::Failed);
    assert!(pending_inputs(&c).unwrap().is_empty());
    start_run(&mut c, "b", "run-rebound");
    assert!(
        mailbox_for_run(&c, &replacement, "run-rebound", &MailboxQuery::default()).unwrap()
            ["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn workflow_execution_stop_rejects_new_mutation_but_retry_keeps_success_receipt() {
    let mut c = fixture(false);
    let req = request(&c, "send", &[("b", "delivered")]);
    let receipt = send(&mut c, &req).unwrap();
    let chat = conversation(&c, "a");
    pause_conversation(&mut c, &chat).unwrap();
    assert_eq!(send(&mut c, &req).unwrap().id, receipt.id);
    let another = request(&c, "other", &[("b", "blocked")]);
    assert!(send(&mut c, &another).is_err());
}
#[test]
fn workflow_execution_reply_reference_must_belong_to_both_participants() {
    let mut c = fixture(false);
    let receipt = send_mail(&mut c, "send", &[("b", "question")]);
    let mut req = request(&c, "reply", &[("c", "private reply")]);
    req.messages[0].reply_to_message_id = Some(receipt.messages[0].id.clone());
    assert!(send(&mut c, &req).is_err());
    req.messages[0].target_node_id = "b".into();
    assert!(send(&mut c, &req).is_ok());
}
#[test]
fn workflow_execution_terminal_trace_settlement_is_atomic_and_preserves_explicit_completion() {
    use crate::storage::conversation_trace_repository::{
        commit_trace_in_connection, get_base_trace_for_message,
    };
    use crate::ConversationTurnTraceTerminalStatus;
    let mut c = fixture(false);
    let receipt = send_mail(
        &mut c,
        "send",
        &[("b", "finish automatically"), ("b", "finish explicitly")],
    );
    start_run(&mut c, "b", "run-b");
    let accept = action(
        &c,
        "b",
        "run-b",
        "accept",
        MutationAction::Accept,
        &receipt
            .messages
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
    );
    mutate(&mut c, &accept).unwrap();
    for input in &receipt.input_ids {
        prove(&c, input);
        mark_applied(&mut c, input).unwrap();
    }
    let complete = action(
        &c,
        "b",
        "run-b",
        "complete",
        MutationAction::Complete,
        &[receipt.messages[1].id.clone()],
    );
    mutate(&mut c, &complete).unwrap();
    let mut trace = get_base_trace_for_message(&c, "run-b").unwrap().unwrap();
    trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
    {
        let tx = c.transaction().unwrap();
        commit_trace_in_connection(&tx, &trace, 1, 2).unwrap();
        assert_eq!(
            load_input(&tx, &receipt.input_ids[0])
                .unwrap()
                .unwrap()
                .mail_status,
            MailStatus::Processed
        );
        tx.rollback().unwrap();
    }
    assert_eq!(status(&c, &receipt, 0), MailStatus::Processing);
    assert_eq!(status(&c, &receipt, 1), MailStatus::Processed);
    {
        let tx = c.transaction().unwrap();
        commit_trace_in_connection(&tx, &trace, 1, 2).unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(status(&c, &receipt, 0), MailStatus::Processed);
    // A late terminal callback cannot undo work explicitly completed in this turn.
    settle_run(&c, "run-b", "cancelled").unwrap();
    assert_eq!(status(&c, &receipt, 1), MailStatus::Processed);
}
#[test]
fn workflow_execution_claim_uses_current_context_without_rewriting_original_envelope() {
    let mut c = fixture(false);
    let receipt = send_mail(
        &mut c,
        "send",
        &[("b", "original body"), ("b", "manual body")],
    );
    let mut definition: Value = serde_json::from_str(
        &c.query_row(
            "SELECT definition_json FROM workflow_definitions WHERE workflow_id='template'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    definition["nodes"][1]["name"] = json!("New recipient");
    c.execute("UPDATE workflow_definitions SET definition_json=?1,revision=revision+1 WHERE workflow_id='template'",[definition.to_string()]).unwrap();
    c.execute("UPDATE workflow_instances SET name='Renamed workflow',template_revision=template_revision+1 WHERE instance_id='instance'",[]).unwrap();
    start_run(&mut c, "b", "run-b");
    bind_input(&mut c, &receipt.input_ids[0], "run-b", "initial").unwrap();
    let accept = action(
        &c,
        "b",
        "run-b",
        "accept",
        MutationAction::Accept,
        &[receipt.messages[1].id.clone()],
    );
    mutate(&mut c, &accept).unwrap();
    for id in &receipt.input_ids {
        let input = load_input(&c, id).unwrap().unwrap();
        assert!(input.content.contains("Recipient: New recipient"));
        assert!(input.content.contains("Workflow: Renamed workflow"));
        assert_eq!(input.messages[0].workflow_name, "Review workflow");
        assert_eq!(input.messages[0].target_node_name, "b");
        prove(&c, id);
        mark_applied(&mut c, id).unwrap();
    }
}
#[test]
fn workflow_execution_manual_completion_gets_one_later_terminal_refresh_event() {
    use crate::storage::conversation_trace_repository::{
        get_base_trace_for_message, replace_trace,
    };
    use crate::ConversationTurnTraceTerminalStatus;
    let mut c = fixture(false);
    let receipt = send_mail(&mut c, "send", &[("b", "work before final answer")]);
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
    let complete = action(
        &c,
        "b",
        "run-b",
        "complete",
        MutationAction::Complete,
        &[receipt.messages[0].id.clone()],
    );
    mutate(&mut c, &complete).unwrap();
    assert_eq!(status(&c, &receipt, 0), MailStatus::Processed);
    mark_run_unread(&mut c, "run-b").unwrap();
    assert!(runtime_snapshot(&c, "instance", None)
        .unwrap()
        .events
        .iter()
        .all(|event| event.kind != "run_completed"));
    let mut trace = get_base_trace_for_message(&c, "run-b").unwrap().unwrap();
    trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
    replace_trace(&mut c, &trace, 1, 2).unwrap();
    mark_run_unread(&mut c, "run-b").unwrap();
    let snapshot = runtime_snapshot(&c, "instance", None).unwrap();
    let completed: Vec<_> = snapshot
        .events
        .iter()
        .filter(|event| event.kind == "run_completed")
        .collect();
    assert_eq!(completed.len(), 1);
    assert_eq!(
        completed[0].input_id.as_deref(),
        Some(receipt.input_ids[0].as_str())
    );
    assert!(completed[0].source_node_id.is_none() && completed[0].target_node_id.is_none());
    assert_eq!(status(&c, &receipt, 0), MailStatus::Processed);
    assert_eq!(
        c.query_row(
            "SELECT completion_notified FROM workflow_mail_inputs WHERE input_id=?1",
            [&receipt.input_ids[0]],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    mark_run_unread(&mut c, "run-b").unwrap();
    assert_eq!(
        runtime_snapshot(&c, "instance", None).unwrap().sequence,
        snapshot.sequence
    );
}
#[test]
fn workflow_execution_snapshot_includes_old_event_references_without_unbounded_history_or_bodies() {
    use crate::storage::conversation_trace_repository::{
        get_base_trace_for_message, replace_trace,
    };
    use crate::ConversationTurnTraceTerminalStatus;
    let mut c = fixture(true);
    let receipt = send_mail(&mut c, "old-mail", &[("b", "private original body")]);
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
    let complete = action(
        &c,
        "b",
        "run-b",
        "complete",
        MutationAction::Complete,
        &[receipt.messages[0].id.clone()],
    );
    mutate(&mut c, &complete).unwrap();
    // Completed history is deliberately larger than the normal 128-row monitor window.
    for call in ["newer-1", "newer-2"] {
        let newer = send_mail(&mut c, call, &vec![("human", "other private body"); 80]);
        for input in newer.input_ids {
            complete_user_input(&mut c, &input).unwrap();
        }
    }
    let boundary = runtime_snapshot(&c, "instance", None).unwrap().sequence;
    let before = runtime_snapshot(&c, "instance", Some(boundary)).unwrap();
    assert_eq!(before.inputs.len(), 128);
    assert!(before
        .inputs
        .iter()
        .all(|input| input.id != receipt.input_ids[0]));
    let mut trace = get_base_trace_for_message(&c, "run-b").unwrap().unwrap();
    trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
    replace_trace(&mut c, &trace, 1, 2).unwrap();
    mark_run_unread(&mut c, "run-b").unwrap();
    let after = runtime_snapshot(&c, "instance", Some(boundary)).unwrap();
    assert_eq!(after.events.len(), 1);
    assert_eq!(after.events[0].kind, "run_completed");
    assert_eq!(after.inputs.len(), 129);
    assert!(after
        .inputs
        .iter()
        .any(|input| input.id == receipt.input_ids[0]
            && input.conversation_id == Some(conversation(&c, "b"))));
    assert!(after.inputs.iter().all(|input| input.content.is_empty()
        && input
            .messages
            .iter()
            .all(|message| message.content.is_empty())));
    let caught_up = runtime_snapshot(&c, "instance", Some(after.sequence)).unwrap();
    assert!(caught_up.events.is_empty());
    assert_eq!(caught_up.inputs.len(), 128);
}
