use super::fixtures::*;
use super::*;

#[test]
fn malformed_current_pending_rows_with_noncanonical_identity_retire_without_dispatch() {
    for expected_status in ["pending", "approved", "executing"] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let conversation_id = format!("conversation-malformed-{expected_status}");
        let assistant_message_id = format!("assistant-malformed-{expected_status}");
        let action_id = format!("action-malformed-{expected_status}");
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        let trace = seed_current_in_progress_tool_trace(
            &service,
            "run-1",
            &conversation_id,
            &assistant_message_id,
            &action_id,
            "run_command",
        );
        if expected_status == "executing" {
            let call = AgentToolCall {
                id: action_id.clone(),
                tool: "run_command".to_string(),
                args: serde_json::json!({}),
                approval_status: crate::AgentApprovalStatus::Approved,
                reason: None,
            };
            let result = AgentToolResult {
                exact_archive_file: None,
                call_id: action_id.clone(),
                tool: "run_command".to_string(),
                ok: false,
                result: Some(serde_json::json!({ "outcome": "unknown" })),
                error: Some("outcome unknown".to_string()),
            };
            let mut closed_trace = trace.clone();
            closed_trace
                .items
                .push(crate::conversation_trace::projected_tool_result_trace_item(
                    1, &call, &result,
                ));
            service
                .append_in_progress_conversation_turn_trace_and_apply_guidances(
                    &closed_trace,
                    &current_model_context_for_trace(&closed_trace),
                    1,
                    2,
                )
                .unwrap();
        } else {
            assert!(trace
                .committed_model_context_prefix(
                    &service
                        .get_conversation_model_context_log(&assistant_message_id)
                        .unwrap()
                        .unwrap()
                        .items,
                )
                .unwrap()
                .is_empty());
        }

        let mut pending = pending_action(&action_id, &conversation_id);
        pending.assistant_message_id = Some(assistant_message_id.clone());
        pending.status = expected_status.to_string();
        pending.action_json = serde_json::json!({ "unknownAction": true }).to_string();
        pending.agent_input_json = serde_json::json!({ "unknownResume": true }).to_string();
        service.store_pending_agent_action(pending).unwrap();
        let mut audit = action_audit(&action_id, &conversation_id);
        audit.assistant_message_id = Some(assistant_message_id.clone());
        audit.status = expected_status.to_string();
        audit.action_json =
            serde_json::json!({ "privateActionCanary": "PRIVATE_ACTION_CANARY" }).to_string();
        audit.completed_at = None;
        service.upsert_agent_action_audit(audit).unwrap();
        let mut usage = agent_usage_record(&conversation_id, &assistant_message_id);
        usage.status = Some("running".to_string());
        usage.completed_at = None;
        service.upsert_agent_usage(usage).unwrap();

        assert!(service
            .retire_unsupported_or_malformed_pending_agent_action_on_startup(
                &action_id,
                expected_status,
                42,
            )
            .unwrap());

        let connection = service.state.connection().unwrap();
        let pending_state: (String, Option<String>, String, String) = connection
            .query_row(
                "SELECT status, target_status, action_json, agent_input_json
                 FROM agent_pending_actions WHERE action_id = ?1",
                [&action_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            pending_state,
            (
                "failed".to_string(),
                Some("failed".to_string()),
                "{}".to_string(),
                "{}".to_string(),
            )
        );
        let audit_state: (String, String, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT status, action_json, error, blocked_reason
                 FROM agent_action_audit WHERE action_id = ?1",
                [&action_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(audit_state.0, "failed");
        assert_eq!(audit_state.1, "{}");
        assert!(!audit_state.1.contains("PRIVATE_ACTION_CANARY"));
        if expected_status == "executing" {
            assert_eq!(
                audit_state.2.as_deref(),
                Some("agent.pending_action_outcome_unknown")
            );
            assert!(audit_state.3.unwrap().contains("outcome is unknown"));
        } else {
            assert_eq!(
                audit_state.2.as_deref(),
                Some("agent.pending_action_unsupported_or_malformed")
            );
            assert!(audit_state.3.unwrap().contains("before dispatch"));
        }
        drop(connection);

        let terminal_trace = service
            .get_conversation_turn_trace(&assistant_message_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            terminal_trace.terminal_status,
            crate::ConversationTurnTraceTerminalStatus::Failed
        );
        let terminal_model_context = service
            .get_conversation_model_context_log(&assistant_message_id)
            .unwrap()
            .unwrap();
        terminal_trace
            .validate_complete_model_context(&terminal_model_context.items)
            .unwrap();
        assert_eq!(
            terminal_trace
                .items
                .iter()
                .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn malformed_pending_retirement_fails_before_mutation_without_trace_or_exact_context() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-malformed-boundary";
    let assistant_message_id = "assistant-malformed-boundary";
    let action_id = "action-malformed-boundary";
    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    let mut pending = pending_action(action_id, conversation_id);
    pending.assistant_message_id = Some(assistant_message_id.to_string());
    let private_action =
        serde_json::json!({ "privateActionCanary": "PRIVATE_ACTION_CANARY" }).to_string();
    let private_resume =
        serde_json::json!({ "privateResumeCanary": "PRIVATE_RESUME_CANARY" }).to_string();
    pending.action_json = private_action.clone();
    pending.agent_input_json = private_resume.clone();
    service.store_pending_agent_action(pending).unwrap();

    assert!(service
        .retire_unsupported_or_malformed_pending_agent_action_on_startup(action_id, "pending", 42)
        .unwrap_err()
        .contains("durable ConversationTurnTrace"));

    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-1".to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: action_id.to_string(),
            tool: "run_command".to_string(),
            provenance: crate::AgentToolIdentity::Builtin {
                tool_name: "run_command".to_string(),
            },
            operation: serde_json::json!({}),
            approval_status: crate::AgentApprovalStatus::Required,
            truncated: false,
        }],
    };
    service
        .append_in_progress_conversation_turn_trace(&trace, 1, 1)
        .unwrap();
    assert!(service
        .retire_unsupported_or_malformed_pending_agent_action_on_startup(action_id, "pending", 43)
        .unwrap_err()
        .contains("model-context log"));

    let connection = service.state.connection().unwrap();
    let unchanged: (String, Option<String>, String, String) = connection
        .query_row(
            "SELECT status, target_status, action_json, agent_input_json
             FROM agent_pending_actions WHERE action_id = ?1",
            [action_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        unchanged,
        ("pending".to_string(), None, private_action, private_resume,)
    );
}
