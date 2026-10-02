//! Read an existing durable result without reauthorizing a new side effect. The Host binds every
//! query to the same conversation, run and tool call that originally committed the result.
use super::*;
use crate::{WorkflowMailReceipt, WorkflowMailReceiptCall};

pub fn mail_receipt_for_call(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
    tool_call_id: &str,
    call: &WorkflowMailReceiptCall,
) -> Result<Option<WorkflowMailReceipt>, String> {
    let sql = match call {
        WorkflowMailReceiptCall::Send { .. } | WorkflowMailReceiptCall::SemanticSend { .. } => "SELECT request_json,receipt_json FROM workflow_mail_sends WHERE source_run_id=?1 AND tool_call_id=?2",
        WorkflowMailReceiptCall::Mutation { .. } => "SELECT request_json,receipt_json FROM workflow_mail_mutations WHERE source_run_id=?1 AND tool_call_id=?2",
    };
    let row: Option<(String, String)> = c
        .query_row(sql, params![run_id, tool_call_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(db)?;
    let Some((request, receipt)) = row else {
        return Ok(None);
    };
    let request: Value = parse(&request)?;
    let same_input = match call {
        WorkflowMailReceiptCall::SemanticSend { input } => request.get("modelInput") == Some(input),
        WorkflowMailReceiptCall::Send { messages } => {
            request["messages"] == serde_json::to_value(messages).map_err(db)?
        }
        WorkflowMailReceiptCall::Mutation {
            action,
            message_ids,
        } => {
            request["action"] == serde_json::to_value(action).map_err(db)?
                && request["messageIds"] == serde_json::to_value(message_ids).map_err(db)?
        }
    };
    if request["conversationId"] != conversation_id
        || request["sourceRunId"] != run_id
        || request["toolCallId"] != tool_call_id
        || !same_input
    {
        return Err("Organization call identity was reused with different arguments".into());
    }
    // executionVersion is a precondition for new effects. This run's original committed call
    // remains recoverable even when its membership incarnation has since been removed/rebound.
    Ok(Some(match call {
        WorkflowMailReceiptCall::Send { .. } | WorkflowMailReceiptCall::SemanticSend { .. } => {
            WorkflowMailReceipt::Send(parse(&receipt)?)
        }
        WorkflowMailReceiptCall::Mutation { .. } => WorkflowMailReceipt::Mutation(parse(&receipt)?),
    }))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{action, conversation, fixture, prove, request, start_run};
    use super::*;

    fn named_request(c: &Connection, call: &str) -> SendRequest {
        let mut req = request(c, call, &[("b", "Review this")]);
        req.model_input =
            Some(serde_json::json!({"messages":[{"to":"b","message":"Review this"}]}));
        let directory = snapshot_for_run(c, &req.conversation_id, &req.source_run_id)
            .unwrap()
            .unwrap();
        req.recipient_versions = directory
            .members
            .into_iter()
            .filter_map(|member| {
                member
                    .membership_version
                    .map(|version| (member.node_id, version))
            })
            .collect();
        req
    }

    #[test]
    fn semantic_send_replays_original_arguments_even_after_identity_replacement() {
        let mut c = fixture();
        let req = named_request(&c, "semantic-send");
        let receipt = send(&mut c, &req).unwrap();
        c.execute(
            "UPDATE workflow_instance_bindings SET membership_id='replacement' WHERE node_id='b'",
            [],
        )
        .unwrap();
        let query = WorkflowMailReceiptCall::SemanticSend {
            input: req.model_input.clone().unwrap(),
        };
        let Some(WorkflowMailReceipt::Send(replay)) = mail_receipt_for_call(
            &c,
            &req.conversation_id,
            &req.source_run_id,
            &req.tool_call_id,
            &query,
        )
        .unwrap() else {
            panic!("missing original receipt")
        };
        assert_eq!(replay.id, receipt.id);
        let mut concurrent_retry = req.clone();
        concurrent_retry.messages[0].target_node_id = "c".into();
        concurrent_retry.execution_version = "a later observation".into();
        assert_eq!(send(&mut c, &concurrent_retry).unwrap().id, receipt.id);
        let altered = WorkflowMailReceiptCall::SemanticSend {
            input: serde_json::json!({"messages":[{"to":"c","message":"Review this"}]}),
        };
        assert!(mail_receipt_for_call(
            &c,
            &req.conversation_id,
            &req.source_run_id,
            &req.tool_call_id,
            &altered
        )
        .is_err());
        let mut stale = req;
        stale.tool_call_id = "different-call".into();
        assert!(send(&mut c, &stale)
            .unwrap_err()
            .contains("recipient changed"));
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM workflow_mail_sends", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn semantic_send_follows_original_member_after_rename_but_not_a_reused_name() {
        let mut c = fixture();
        let req = named_request(&c, "rename-target");
        let mut definition: Value = c
            .query_row(
                "SELECT definition_json FROM workflow_instances WHERE instance_id='instance'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|raw| serde_json::from_str(&raw).unwrap())
            .unwrap();
        definition["nodes"][1]["name"] = serde_json::json!("Renamed reviewer");
        definition["nodes"][2]["name"] = serde_json::json!("b");
        c.execute(
            "UPDATE workflow_instances SET definition_json=?1 WHERE instance_id='instance'",
            [definition.to_string()],
        )
        .unwrap();
        let receipt = send(&mut c, &req).unwrap();
        assert_eq!(receipt.messages[0].target_node_id, "b");
        assert_eq!(receipt.messages[0].target_node_name, "Renamed reviewer");
    }

    #[test]
    fn workflow_committed_mail_receipts_survive_member_removal_without_authorizing_new_calls() {
        let mut c = fixture();
        let send_request = request(
            &c,
            "send-before-removal",
            &[("b", "Complete this"), ("b", "Recall this")],
        );
        let sent = send(&mut c, &send_request).unwrap();
        start_run(&mut c, "b", "run-b");
        let accepted_request = action(
            &c,
            "b",
            "run-b",
            "accept-before-removal",
            MutationAction::Accept,
            &[sent.messages[0].id.clone()],
        );
        let accepted = mutate(&mut c, &accepted_request).unwrap();
        prove(&c, &sent.input_ids[0]);
        let completed_request = action(
            &c,
            "b",
            "run-b",
            "complete-before-removal",
            MutationAction::Complete,
            &[sent.messages[0].id.clone()],
        );
        let completed = mutate(&mut c, &completed_request).unwrap();
        let recalled_request = action(
            &c,
            "a",
            "run-a",
            "recall-before-removal",
            MutationAction::Recall,
            &[sent.messages[1].id.clone()],
        );
        let recalled = mutate(&mut c, &recalled_request).unwrap();
        assert_eq!(completed["messages"][0]["status"], "processed");
        assert_eq!(recalled["messages"][0]["status"], "recalled");
        let sender = conversation(&c, "a");
        c.execute(
            "DELETE FROM workflow_instance_bindings WHERE instance_id='instance'",
            [],
        )
        .unwrap();
        let query = WorkflowMailReceiptCall::Send {
            messages: send_request.messages.clone(),
        };
        let Some(WorkflowMailReceipt::Send(recovered)) =
            mail_receipt_for_call(&c, &sender, "run-a", "send-before-removal", &query).unwrap()
        else {
            panic!("missing committed send")
        };
        assert_eq!(json(&recovered).unwrap(), json(&sent).unwrap());
        for (request, result) in [
            (&accepted_request, accepted),
            (&completed_request, completed),
            (&recalled_request, recalled),
        ] {
            let call = WorkflowMailReceiptCall::Mutation {
                action: request.action,
                message_ids: request.message_ids.clone(),
            };
            let Some(WorkflowMailReceipt::Mutation(recovered)) = mail_receipt_for_call(
                &c,
                &request.conversation_id,
                &request.source_run_id,
                &request.tool_call_id,
                &call,
            )
            .unwrap() else {
                panic!("missing committed mutation")
            };
            assert_eq!(recovered, result);
            assert!(mail_receipt_for_call(
                &c,
                &request.conversation_id,
                &request.source_run_id,
                "new-call",
                &call
            )
            .unwrap()
            .is_none());
            let mut new = request.clone();
            new.tool_call_id = "new-call".into();
            assert!(mutate(&mut c, &new).is_err());
        }
        let mut new_send = send_request.clone();
        new_send.tool_call_id = "new-send".into();
        assert!(send(&mut c, &new_send).is_err());
        assert!(mail_receipt_for_call(
            &c,
            "another-conversation",
            "run-a",
            "send-before-removal",
            &query
        )
        .is_err());
        let altered = WorkflowMailReceiptCall::Send {
            messages: vec![SendOutput {
                target_node_id: "b".into(),
                message: "Different".into(),
                reply_to_message_id: None,
            }],
        };
        assert!(
            mail_receipt_for_call(&c, &sender, "run-a", "send-before-removal", &altered).is_err()
        );
        let altered = WorkflowMailReceiptCall::Mutation {
            action: MutationAction::Recall,
            message_ids: completed_request.message_ids,
        };
        assert!(mail_receipt_for_call(
            &c,
            &completed_request.conversation_id,
            "run-b",
            "complete-before-removal",
            &altered
        )
        .is_err());
    }
}
