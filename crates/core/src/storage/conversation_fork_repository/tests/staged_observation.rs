#[test]
fn fork_reuses_one_observation_mapping_for_two_staged_transactions_recursively() {
    let mut fixture = current_apply_patch_staged_fork_fixture("draft\n", "after\n");
    let connection = &mut fixture.connection;
    let second_transaction_id = "file-change-second";
    let second_calls = [BEGIN_CALL_ID, APPEND_CALL_ID, EDIT_CALL_ID, COMMIT_CALL_ID]
        .map(|id| (id.to_string(), format!("second-{id}")))
        .into_iter()
        .collect::<HashMap<_, _>>();
    let mut raw_trace =
        conversation_trace_repository::get_trace_for_message(connection, "assistant-b")
            .unwrap()
            .unwrap();
    let context =
        conversation_model_context_repository::get_log_for_message(connection, "assistant-b")
            .unwrap()
            .unwrap();
    // Reconstruct runtime inputs from the exact model log before passing both transactions
    // through the recorder. Never treat the body-free audit projection as a raw request.
    for item in &mut raw_trace.items {
        let model_item = context
            .items
            .iter()
            .find(|model| model.sequence == item.sequence())
            .unwrap();
        match item {
            ConversationTurnTraceItem::ToolCall { operation, .. } => {
                *operation = model_item.tool_calls[0].args.clone()
            }
            ConversationTurnTraceItem::ToolResult { observation, .. } => {
                *observation = serde_json::from_str(&model_item.content).unwrap()
            }
            _ => unreachable!(),
        }
    }
    let mut second_items = raw_trace.items[2..].to_vec();
    let mut second_args = HashMap::new();
    for (index, item) in second_items.iter_mut().enumerate() {
        match item {
            ConversationTurnTraceItem::ToolCall {
                sequence,
                call_id,
                operation,
                ..
            } => {
                *sequence = 10 + index as u64;
                *call_id = second_calls[call_id].clone();
                if let Some(transaction_id) = operation["request"].get_mut("transactionId") {
                    *transaction_id = json!(second_transaction_id);
                }
                second_args.insert(call_id.clone(), operation.clone());
            }
            ConversationTurnTraceItem::ToolResult {
                sequence, call_id, ..
            } => {
                *sequence = 10 + index as u64;
                *call_id = second_calls[call_id].clone();
            }
            _ => unreachable!(),
        }
    }
    raw_trace.items.extend(second_items);
    // This only rebuilds this in-memory test fixture before the first fork attempt.
    connection
        .execute(
            "DELETE FROM conversation_model_context_items WHERE assistant_message_id='assistant-b'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM conversation_turn_traces WHERE assistant_message_id='assistant-b'",
            [],
        )
        .unwrap();
    commit_current_fork_fixture_trace(connection, &raw_trace, 40, 41);

    let mut second_change =
        file_change_repository::get_file_change(connection, SOURCE_TRANSACTION_ID)
            .unwrap()
            .unwrap();
    second_change.id = second_transaction_id.to_string();
    second_change.source_tool_call_id = second_calls[BEGIN_CALL_ID].clone();
    second_change.source_tool_arguments_digest =
        crate::file_change::proposal_digest(&second_args[&second_calls[BEGIN_CALL_ID]]).unwrap();
    second_change.final_action_id = Some(second_calls[COMMIT_CALL_ID].clone());
    second_change.final_action_arguments_digest = Some(
        crate::file_change::proposal_digest(&second_args[&second_calls[COMMIT_CALL_ID]]).unwrap(),
    );
    // A successful write can renew its existing observation; both begin calls reference it.
    assert_eq!(second_change.observation_id, fixture.observation_id);
    file_change_repository::insert_file_change(connection, &second_change).unwrap();
    let mut second_history = file_change_repository::load_file_change_history_snapshot(
        connection,
        SOURCE_TRANSACTION_ID,
        None,
    )
    .unwrap();
    for chunk in &mut second_history.chunks {
        chunk.transaction_id = second_transaction_id.to_string();
    }
    for operation in &mut second_history.operations {
        operation.transaction_id = second_transaction_id.to_string();
        operation.source_tool_call_id = second_calls[&operation.source_tool_call_id].clone();
        operation.source_tool_arguments_digest =
            crate::file_change::proposal_digest(&second_args[&operation.source_tool_call_id])
                .unwrap();
        operation.payload_digest = operation.source_tool_arguments_digest.clone();
        let mut receipt: crate::file_change::FileChangeMutationReceipt =
            serde_json::from_str(&operation.receipt_json).unwrap();
        receipt.transaction_id = second_transaction_id.to_string();
        operation.receipt_json = serde_json::to_string(&receipt).unwrap();
    }
    file_change_repository::insert_file_change_history_snapshot(
        connection,
        second_transaction_id,
        &second_history,
    )
    .unwrap();
    let mut audit = agent_action_audit_repository::load_action_audit_record(
        connection,
        &crate::canonical_pending_action_id("run-source-1", COMMIT_CALL_ID),
    )
    .unwrap()
    .unwrap();
    let mut action: AgentProposedAction = serde_json::from_str(&audit.action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = &mut action else {
        unreachable!()
    };
    let commit_id = &second_calls[COMMIT_CALL_ID];
    file_change.id = commit_id.clone();
    file_change.transaction_id = second_transaction_id.to_string();
    let execution = &mut file_change.execution;
    execution.transaction.id = second_transaction_id.to_string();
    execution.proposal.id = commit_id.clone();
    execution.proposal.transaction_id = second_transaction_id.to_string();
    execution.source_call_id = commit_id.clone();
    execution.staged_transaction_id = Some(second_transaction_id.to_string());
    execution.source_args_digest =
        crate::file_change::proposal_digest(&second_args[commit_id]).unwrap();
    execution.trace_args_digest =
        crate::file_change_support::apply_patch_trace_args_digest(&second_args[commit_id]).unwrap();
    file_change.validate().unwrap();
    audit.action_id = crate::canonical_pending_action_id("run-source-1", commit_id);
    audit.action_json = serde_json::to_string(&action).unwrap();
    assert!(
        agent_action_audit_repository::insert_action_audit_record_if_absent(connection, &audit)
            .unwrap()
    );

    let mut conversation_id = fixture.source.id.clone();
    let mut message_id = "assistant-c".to_string();
    let mut tool_message_id = "assistant-b".to_string();
    for generation in 0..2 {
        let plan = build_assistant_reply_fork_plan(
            connection,
            &format!("fork-shared-observation-{generation}"),
            &conversation_id,
            &message_id,
            100 + generation,
        )
        .unwrap();
        assert_eq!(plan.file_changes.len(), 2);
        assert_eq!(plan.action_audits.len(), 2);
        assert_eq!(
            plan.file_changes[0].observation_id,
            plan.file_changes[1].observation_id
        );
        commit_fork_plan(connection, &plan).unwrap();
        let context = conversation_model_context_repository::get_log_for_message(
            connection,
            &plan.message_id_map[&tool_message_id],
        )
        .unwrap()
        .unwrap();
        let calls = context
            .items
            .iter()
            .flat_map(|item| &item.tool_calls)
            .map(|call| (call.id.as_str(), &call.args))
            .collect::<HashMap<_, _>>();
        for change in &plan.file_changes {
            let begin_args = calls[change.source_tool_call_id.as_str()];
            assert_eq!(
                begin_args["request"]["observationId"],
                change.observation_id
            );
            assert_eq!(
                crate::file_change::proposal_digest(begin_args).unwrap(),
                change.source_tool_arguments_digest
            );
        }
        conversation_id = plan.target.id.clone();
        message_id = plan.message_id_map[&message_id].clone();
        tool_message_id = plan.message_id_map[&tool_message_id].clone();
    }
}
