use super::command_fixture::manual_command_settlement;
use super::fixtures::*;
use super::non_command_fixture::{
    manual_non_command_file_effect_settlement, ManualNonCommandFileEffect,
};
use super::*;

#[derive(Debug, Clone, Copy)]
enum ManualTraceReceiptTamper {
    Observation,
    Error,
    CallApproval,
}

fn assert_tampered_manual_trace_receipt_remains_unsettled(
    mut pending: AgentPendingActionRecord,
    approved: AgentActionAuditRecord,
    terminal: AgentActionAuditRecord,
    mut trace: ConversationTurnTrace,
    tamper: ManualTraceReceiptTamper,
) {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let storage_id = pending.action_id.clone();
    let run_id = pending.run_id.clone();
    let conversation_id = pending.conversation_id.clone().unwrap();
    let assistant_message_id = pending.assistant_message_id.clone().unwrap();
    attach_current_manual_file_effect_checkpoint(&mut pending, &trace);
    save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();
    service
        .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
        .unwrap();
    assert_eq!(
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap()
            .len(),
        1
    );
    assert!(service.list_unsettled_file_effects().unwrap().is_empty());

    let tampered_item = match tamper {
        ManualTraceReceiptTamper::Observation | ManualTraceReceiptTamper::Error => {
            let result_item = trace
                .items
                .iter_mut()
                .find(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
                .unwrap();
            let ConversationTurnTraceItem::ToolResult {
                observation, error, ..
            } = result_item
            else {
                unreachable!("selected trace item must be a ToolResult");
            };
            match tamper {
                ManualTraceReceiptTamper::Observation => {
                    *observation = serde_json::json!({ "tampered": true });
                }
                ManualTraceReceiptTamper::Error => {
                    *error = Some("tampered trace error".to_string());
                }
                ManualTraceReceiptTamper::CallApproval => unreachable!(),
            }
            result_item
        }
        ManualTraceReceiptTamper::CallApproval => {
            let call_item = trace
                .items
                .iter_mut()
                .find(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
                .unwrap();
            let ConversationTurnTraceItem::ToolCall {
                approval_status, ..
            } = call_item
            else {
                unreachable!("selected trace item must be a ToolCall");
            };
            *approval_status = crate::AgentApprovalStatus::Rejected;
            call_item
        }
    };
    let sequence = tampered_item.sequence();
    let item_json = serde_json::to_string(tampered_item).unwrap();
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE conversation_turn_trace_items
             SET item_json = ?3
             WHERE assistant_message_id = ?1 AND sequence = ?2",
            rusqlite::params![assistant_message_id, sequence, item_json],
        )
        .unwrap();

    assert!(service
        .reconcile_interrupted_pending_agent_actions(43)
        .unwrap()
        .is_empty());
    let expected = AgentUnsettledFileEffect {
        project_id: Some("project-1".to_string()),
        conversation_id,
        run_id,
        action_id: storage_id,
    };
    assert_eq!(
        service.list_unsettled_file_effects().unwrap(),
        vec![expected.clone()]
    );
    let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    assert_eq!(
        reopened.list_unsettled_file_effects().unwrap(),
        vec![expected]
    );
}

#[test]
fn tampered_trace_evidence_never_settles_a_manual_file_effect() {
    for tamper in [
        ManualTraceReceiptTamper::Observation,
        ManualTraceReceiptTamper::Error,
        ManualTraceReceiptTamper::CallApproval,
    ] {
        let settlement = manual_command_settlement(
            "tampered-trace-command",
            "conversation-tampered-trace-command",
            "assistant-tampered-trace-command",
        );
        assert_tampered_manual_trace_receipt_remains_unsettled(
            settlement.0,
            settlement.1,
            settlement.2,
            settlement.3,
            tamper,
        );

        for effect in ManualNonCommandFileEffect::ALL {
            let label = effect.label();
            let call_id = format!("tampered-trace-{label}-call");
            let run_id = format!("run-tampered-trace-{label}");
            let settlement = manual_non_command_file_effect_settlement(
                effect,
                &call_id,
                &run_id,
                &format!("conversation-tampered-trace-{label}"),
                &format!("assistant-tampered-trace-{label}"),
            );
            assert_tampered_manual_trace_receipt_remains_unsettled(
                settlement.0,
                settlement.1,
                settlement.2,
                settlement.3,
                tamper,
            );
        }
    }
}

#[test]
fn tampered_command_trace_arguments_restore_the_durable_blocker() {
    let base_operation = serde_json::json!({
        "command": "node script.mjs",
        "reason": "test atomic settlement",
    });
    let mut cases = Vec::new();
    for (label, field, value) in [
        ("command", "command", serde_json::json!("node other.mjs")),
        ("cwd", "cwd", serde_json::json!("other")),
        ("timeout", "timeoutMs", serde_json::json!(1)),
        ("reason", "reason", serde_json::json!("different reason")),
    ] {
        let mut operation = base_operation.clone();
        operation[field] = value;
        cases.push((label, operation));
    }
    let mut observe = base_operation.clone();
    observe["observe"] = serde_json::json!({
        "kinds": ["office"],
        "expectedOutputs": ["unexpected.xlsx"],
    });
    cases.push(("observe", observe));
    let mut runtime = base_operation.clone();
    runtime["runtime"] = serde_json::json!({
        "provider": "managedArtifact",
        "kind": "node",
        "requiredPackages": [],
    });
    cases.push(("runtime", runtime));
    let mut unknown = base_operation;
    unknown["executable"] = serde_json::json!("/tmp/untrusted-node");
    cases.push(("unknown", unknown));

    for (label, tampered_operation) in cases {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let call_id = format!("tampered-command-{label}");
        let storage_id = current_pending_storage_id("run-1", &call_id);
        let conversation_id = format!("conversation-tampered-command-{label}");
        let assistant_message_id = format!("assistant-tampered-command-{label}");
        let (mut pending, approved, terminal, mut trace) =
            manual_command_settlement(&call_id, &conversation_id, &assistant_message_id);
        attach_current_manual_file_effect_checkpoint(&mut pending, &trace);
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
            .unwrap();
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();
        assert!(service.list_unsettled_file_effects().unwrap().is_empty());

        let call_item = trace
            .items
            .iter_mut()
            .find(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
            .unwrap();
        let ConversationTurnTraceItem::ToolCall { operation, .. } = call_item else {
            unreachable!("selected trace item must be a ToolCall");
        };
        *operation = tampered_operation;
        service
            .state
            .connection()
            .unwrap()
            .execute(
                "UPDATE conversation_turn_trace_items
                 SET item_json = ?3
                 WHERE assistant_message_id = ?1 AND sequence = ?2",
                rusqlite::params![
                    assistant_message_id,
                    call_item.sequence(),
                    serde_json::to_string(call_item).unwrap(),
                ],
            )
            .unwrap();

        assert_eq!(
            service.list_unsettled_file_effects().unwrap(),
            vec![AgentUnsettledFileEffect {
                project_id: Some("project-1".to_string()),
                conversation_id,
                run_id: "run-1".to_string(),
                action_id: storage_id,
            }],
            "tampered command {label} field was treated as authoritative"
        );
    }
}

#[test]
fn tampered_command_execution_evidence_restores_the_durable_blocker() {
    for tamper_artifact_observation in [false, true] {
        let label = if tamper_artifact_observation {
            "artifact-observation"
        } else {
            "stdout"
        };
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let call_id = format!("tampered-command-result-{label}");
        let storage_id = current_pending_storage_id("run-1", &call_id);
        let conversation_id = format!("conversation-tampered-command-result-{label}");
        let assistant_message_id = format!("assistant-tampered-command-result-{label}");
        let (mut pending, approved, terminal, trace) =
            manual_command_settlement(&call_id, &conversation_id, &assistant_message_id);
        attach_current_manual_file_effect_checkpoint(&mut pending, &trace);
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
            .unwrap();
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();
        assert!(service.list_unsettled_file_effects().unwrap().is_empty());

        let mut command_result = serde_json::from_str::<AgentCommandExecutionResult>(
            terminal.command_result_json.as_deref().unwrap(),
        )
        .unwrap();
        if tamper_artifact_observation {
            command_result.artifact_observation = Some(crate::AgentCommandArtifactObservation {
                schema_version: 1,
                status: crate::AgentCommandArtifactObservationStatus::Complete,
                partial: false,
                stop_reasons: Vec::new(),
                scanned: 0,
                returned: 0,
                omitted: 0,
                coverage: crate::AgentCommandArtifactObservationCoverage {
                    workspace_included: true,
                    expected_output_count: 0,
                    additional_root_count: 0,
                    before: crate::AgentCommandArtifactSnapshotCoverage::default(),
                    after: crate::AgentCommandArtifactSnapshotCoverage::default(),
                },
                changes: Vec::new(),
                changes_truncated: false,
                changes_omitted: 0,
                expected_outputs: Vec::new(),
                warnings: Vec::new(),
            });
        } else {
            command_result.stdout = "tampered stdout".to_string();
        }
        service
            .state
            .connection()
            .unwrap()
            .execute(
                "UPDATE agent_action_audit
                 SET command_result_json = ?2
                 WHERE action_id = ?1",
                rusqlite::params![storage_id, serde_json::to_string(&command_result).unwrap(),],
            )
            .unwrap();

        assert_eq!(
            service.list_unsettled_file_effects().unwrap(),
            vec![AgentUnsettledFileEffect {
                project_id: Some("project-1".to_string()),
                conversation_id,
                run_id: "run-1".to_string(),
                action_id: storage_id,
            }],
            "tampered command {label} evidence was treated as authoritative"
        );
    }
}

#[test]
fn command_terminal_outcome_cannot_diverge_from_execution_evidence() {
    for case in [
        "successful-as-failed",
        "failed-as-successful",
        "failed-as-cancelled",
    ] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let call_id = format!("command-outcome-{case}");
        let storage_id = current_pending_storage_id("run-1", &call_id);
        let conversation_id = format!("conversation-command-outcome-{case}");
        let assistant_message_id = format!("assistant-command-outcome-{case}");
        let (mut pending, approved, terminal, mut trace) =
            manual_command_settlement(&call_id, &conversation_id, &assistant_message_id);
        attach_current_manual_file_effect_checkpoint(&mut pending, &trace);
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
            .unwrap();
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();
        assert!(service.list_unsettled_file_effects().unwrap().is_empty());

        let mut command_result = serde_json::from_str::<AgentCommandExecutionResult>(
            terminal.command_result_json.as_deref().unwrap(),
        )
        .unwrap();
        if case != "successful-as-failed" {
            command_result.exit_code = Some(1);
        }
        let mut tool_result = crate::command::command_tool_result(&call_id, &command_result);
        let target_status = match case {
            "successful-as-failed" => {
                tool_result.ok = false;
                tool_result.error = Some("fabricated command failure".to_string());
                "failed"
            }
            "failed-as-successful" => {
                tool_result.ok = true;
                tool_result.error = None;
                "completed"
            }
            "failed-as-cancelled" => "cancelled",
            _ => unreachable!(),
        };
        let call = AgentToolCall {
            id: call_id.clone(),
            tool: "run_command".to_string(),
            args: serde_json::json!({
                "command": "node script.mjs",
                "reason": "test atomic settlement",
            }),
            approval_status: crate::AgentApprovalStatus::Approved,
            reason: Some("test atomic settlement".to_string()),
        };
        trace.items[1] =
            crate::conversation_trace::projected_tool_result_trace_item(1, &call, &tool_result);

        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "UPDATE agent_pending_actions
                 SET status = ?2, target_status = ?2
                 WHERE action_id = ?1",
                rusqlite::params![storage_id, target_status],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE agent_action_audit
                 SET status = ?2,
                     command_result_json = ?3,
                     tool_result_json = ?4,
                     error = ?5
                 WHERE action_id = ?1",
                rusqlite::params![
                    storage_id,
                    target_status,
                    serde_json::to_string(&command_result).unwrap(),
                    serde_json::to_string(&tool_result).unwrap(),
                    tool_result.error,
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE conversation_turn_trace_items
                 SET item_json = ?3
                 WHERE assistant_message_id = ?1 AND sequence = ?2",
                rusqlite::params![
                    assistant_message_id,
                    1,
                    serde_json::to_string(&trace.items[1]).unwrap(),
                ],
            )
            .unwrap();
        drop(connection);

        let expected = AgentUnsettledFileEffect {
            project_id: Some("project-1".to_string()),
            conversation_id: conversation_id.clone(),
            run_id: "run-1".to_string(),
            action_id: storage_id.clone(),
        };
        assert_eq!(
            service.list_unsettled_file_effects().unwrap(),
            vec![expected.clone()],
            "{case} was treated as an authoritative command settlement"
        );
        let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
        assert_eq!(
            reopened.list_unsettled_file_effects().unwrap(),
            vec![expected]
        );
    }
}
