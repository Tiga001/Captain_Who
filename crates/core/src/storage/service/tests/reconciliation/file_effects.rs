use super::command_fixture::manual_command_settlement;
use super::fixtures::*;
use super::non_command_fixture::{
    manual_non_command_file_effect_settlement, ManualNonCommandFileEffect,
};
use super::*;

fn file_effect_action_type(tool_name: &str) -> &'static str {
    match tool_name {
        "office_document" | "office_spreadsheet" | "office_presentation" => "office_operation",
        "skills_run_script" => "skill_script",
        "skills_materialize_resource" => "skill_materialization",
        other => panic!("unexpected file-producing tool in test: {other}"),
    }
}

fn file_effect_audit(
    storage_id: &str,
    run_id: &str,
    conversation_id: &str,
    tool_name: &str,
    status: &str,
    decision_source: &str,
    created_at: i64,
) -> AgentActionAuditRecord {
    let mut audit = action_audit(storage_id, conversation_id);
    audit.run_id = run_id.to_string();
    audit.action_type = file_effect_action_type(tool_name).to_string();
    audit.tool_name = tool_name.to_string();
    audit.status = status.to_string();
    audit.decision_source = Some(decision_source.to_string());
    audit.created_at = created_at;
    audit.decided_at = Some(created_at);
    audit.completed_at =
        matches!(status, "completed" | "failed" | "cancelled").then_some(created_at + 1);
    audit
}

fn interrupted_file_effect(
    call_id: &str,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    tool_name: &str,
    created_at: i64,
) -> AgentPendingActionRecord {
    let mut pending = current_pending_action(run_id, call_id, conversation_id);
    pending.assistant_message_id = Some(assistant_message_id.to_string());
    pending.action_type = file_effect_action_type(tool_name).to_string();
    pending.tool_name = tool_name.to_string();
    pending.status = "approved".to_string();
    pending.created_at = created_at;
    pending.updated_at = created_at;
    pending
}

#[test]
fn unsettled_effects_restore_every_auto_file_producer_but_exclude_terminal_receipts() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let cases = [
        (
            "office_document",
            "run-auto-document",
            "document-call",
            "conversation-auto-document",
        ),
        (
            "skills_run_script",
            "run-auto-script",
            "script-call",
            "conversation-auto-script",
        ),
        (
            "skills_materialize_resource",
            "run-auto-materialize",
            "materialize-call",
            "conversation-auto-materialize",
        ),
    ];

    for (index, (tool_name, run_id, call_id, conversation_id)) in cases.iter().enumerate() {
        let storage_id = current_pending_storage_id(run_id, call_id);
        let assistant_message_id = format!("assistant-{run_id}");
        save_assistant_conversation(&service, conversation_id, &assistant_message_id);
        service
            .upsert_agent_action_audit(file_effect_audit(
                &storage_id,
                run_id,
                conversation_id,
                tool_name,
                "executing",
                "auto",
                index as i64 + 1,
            ))
            .unwrap();

        let terminal_run_id = format!("{run_id}-terminal");
        let terminal_storage_id =
            current_pending_storage_id(&terminal_run_id, &format!("{call_id}-terminal"));
        service
            .upsert_agent_action_audit(file_effect_audit(
                &terminal_storage_id,
                &terminal_run_id,
                conversation_id,
                tool_name,
                if index % 2 == 0 {
                    "completed"
                } else {
                    "failed"
                },
                "auto",
                index as i64 + 10,
            ))
            .unwrap();
    }

    let mut restored = service
        .list_unsettled_file_effects()
        .unwrap()
        .into_iter()
        .map(|effect| {
            (
                effect.project_id,
                effect.conversation_id,
                effect.run_id,
                effect.action_id,
            )
        })
        .collect::<Vec<_>>();
    restored.sort();

    let mut expected = cases
        .iter()
        .map(|(_, run_id, call_id, conversation_id)| {
            (
                Some("project-1".to_string()),
                (*conversation_id).to_string(),
                (*run_id).to_string(),
                current_pending_storage_id(run_id, call_id),
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(restored, expected);
}

#[test]
fn startup_reconciliation_conservatively_restores_interrupted_manual_file_effects() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let cases = [
        (
            "office_document",
            "run-manual-document",
            "document-call",
            "conversation-manual-document",
            "assistant-manual-document",
        ),
        (
            "skills_run_script",
            "run-manual-script",
            "script-call",
            "conversation-manual-script",
            "assistant-manual-script",
        ),
    ];

    for (index, (tool_name, run_id, call_id, conversation_id, assistant_message_id)) in
        cases.iter().enumerate()
    {
        let storage_id = current_pending_storage_id(run_id, call_id);
        save_assistant_conversation(&service, conversation_id, assistant_message_id);
        let trace = seed_current_in_progress_tool_trace(
            &service,
            run_id,
            conversation_id,
            assistant_message_id,
            call_id,
            tool_name,
        );
        let mut pending = interrupted_file_effect(
            call_id,
            run_id,
            conversation_id,
            assistant_message_id,
            tool_name,
            index as i64 + 1,
        );
        attach_current_manual_file_effect_checkpoint(&mut pending, &trace);
        service.store_pending_agent_action(pending).unwrap();
        service
            .upsert_agent_action_audit(file_effect_audit(
                &storage_id,
                run_id,
                conversation_id,
                tool_name,
                "approved",
                "manual",
                index as i64 + 1,
            ))
            .unwrap();
    }

    let interrupted = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(interrupted.len(), cases.len());

    let mut restored = service
        .list_unsettled_file_effects()
        .unwrap()
        .into_iter()
        .map(|effect| {
            (
                effect.project_id,
                effect.conversation_id,
                effect.run_id,
                effect.action_id,
            )
        })
        .collect::<Vec<_>>();
    restored.sort();
    let mut expected = cases
        .iter()
        .map(|(_, run_id, call_id, conversation_id, _)| {
            (
                Some("project-1".to_string()),
                (*conversation_id).to_string(),
                (*run_id).to_string(),
                current_pending_storage_id(run_id, call_id),
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(restored, expected);

    let connection = service.state.connection().unwrap();
    for (_, run_id, call_id, _, _) in cases {
        let storage_id = current_pending_storage_id(run_id, call_id);
        let state: (String, Option<String>) = connection
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, ("failed".to_string(), Some("failed".to_string())));
    }
}

#[allow(clippy::too_many_arguments)]
fn seed_approved_command_with_terminal_session(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    call_id: &str,
    session_id: &str,
    status: crate::AgentCommandSessionStatus,
    exit_code: Option<i32>,
) {
    use crate::storage::agent_command_session_repository::{
        AgentCommandSessionCreate, AgentCommandSessionTerminalUpdate,
        AGENT_COMMAND_SESSION_SCHEMA_VERSION,
    };
    let (mut pending, approved, _terminal, _trace) =
        manual_command_settlement(call_id, conversation_id, assistant_message_id);
    save_assistant_conversation(service, conversation_id, assistant_message_id);
    let trace = seed_current_in_progress_tool_trace(
        service,
        "run-1",
        conversation_id,
        assistant_message_id,
        call_id,
        "run_command",
    );
    attach_current_manual_file_effect_checkpoint(&mut pending, &trace);
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();
    service
        .create_agent_command_session(&AgentCommandSessionCreate {
            snapshot: crate::AgentCommandSessionSnapshot {
                schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
                session_id: session_id.to_string(),
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                origin_run_id: "run-1".to_string(),
                call_id: call_id.to_string(),
                project_id: Some("project-1".to_string()),
                command: "node script.mjs".to_string(),
                cwd: "/workspace".to_string(),
                command_digest:
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                        .to_string(),
                status: crate::AgentCommandSessionStatus::Starting,
                started_at: 12,
                ended_at: None,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                outputs: Vec::new(),
                artifact_observation: None,
                archive_ref: None,
            },
            authorization_source: crate::command::CommandAuthorizationSource::ExplicitUser,
            approval_provenance: serde_json::json!({"decision": "approved"}),
            permission_provenance: serde_json::json!({"mode": "default"}),
            created_at: 12,
        })
        .unwrap();
    service
        .mark_agent_command_session_running(conversation_id, session_id, 13)
        .unwrap();
    if status == crate::AgentCommandSessionStatus::OutcomeUnknown {
        let reconciled = service
            .reconcile_agent_command_sessions_on_startup(14)
            .unwrap();
        assert!(reconciled
            .iter()
            .any(|record| record.snapshot.session_id == session_id));
        return;
    }
    service
        .settle_agent_command_session(&AgentCommandSessionTerminalUpdate {
            conversation_id,
            session_id,
            status,
            ended_at: 14,
            exit_code,
            latest_sequence: 0,
            transcript_truncated: false,
            output_capture_truncated: false,
            archive_ref: None,
            terminal_reason: Some("missing input PDF"),
            published_outputs: &[],
            artifact_observation: None,
            committed_at: 14,
        })
        .unwrap();
}

#[test]
fn startup_reconciliation_fail_closes_an_approved_command_with_only_a_terminal_session() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-approved-terminal-session";
    let assistant_message_id = "assistant-approved-terminal-session";
    let call_id = "approved-terminal-session";
    let storage_id = current_pending_storage_id("run-1", call_id);
    let session_id = "cmd_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    seed_approved_command_with_terminal_session(
        &service,
        conversation_id,
        assistant_message_id,
        call_id,
        session_id,
        crate::AgentCommandSessionStatus::Exited,
        Some(2),
    );

    // Reproduce the old crash cut exactly: operational Session is terminal, while the approved
    // action never materialized its terminal audit/ToolResult lifecycle. Startup must not replay
    // the command or infer success from the Session alone; it retires the action as failed and
    // retains the failed audit for inspection. The exact terminal Session proves that no process
    // remains active, so it safely releases only the runtime FileEffect fence.
    let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    assert_eq!(
        reopened
            .load_agent_command_session(conversation_id, session_id)
            .unwrap()
            .unwrap()
            .snapshot
            .status,
        crate::AgentCommandSessionStatus::Exited
    );
    assert_eq!(
        reopened
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap()
            .len(),
        1
    );
    let connection = reopened.state.connection().unwrap();
    let state: (String, Option<String>, String, Option<String>) = connection
        .query_row(
            "SELECT pending.status, pending.target_status, audit.status, audit.tool_result_json
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        state,
        (
            "failed".to_string(),
            Some("failed".to_string()),
            "approved".to_string(),
            None,
        )
    );
    drop(connection);
    assert!(reopened.list_unsettled_file_effects().unwrap().is_empty());
    assert!(reopened
        .reconcile_interrupted_pending_agent_actions(43)
        .unwrap()
        .is_empty());
}

#[test]
fn outcome_unknown_command_session_keeps_the_interrupted_file_effect_fence() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-outcome-unknown-session";
    let assistant_message_id = "assistant-outcome-unknown-session";
    let call_id = "outcome-unknown-session";
    let storage_id = current_pending_storage_id("run-1", call_id);
    let session_id = "cmd_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    seed_approved_command_with_terminal_session(
        &service,
        conversation_id,
        assistant_message_id,
        call_id,
        session_id,
        crate::AgentCommandSessionStatus::OutcomeUnknown,
        None,
    );

    let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    assert_eq!(
        reopened
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        reopened.list_unsettled_file_effects().unwrap(),
        vec![AgentUnsettledFileEffect {
            project_id: Some("project-1".to_string()),
            conversation_id: conversation_id.to_string(),
            run_id: "run-1".to_string(),
            action_id: storage_id,
        }]
    );
}

#[test]
fn unsettled_file_effects_preserve_conversation_scope_without_a_project() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    save_assistant_conversation_in_scope(
        &service,
        "conversation-unscoped-auto",
        "assistant-unscoped-auto",
        None,
    );
    let auto_storage_id = current_pending_storage_id("run-unscoped-auto", "auto-call");
    service
        .upsert_agent_action_audit(file_effect_audit(
            &auto_storage_id,
            "run-unscoped-auto",
            "conversation-unscoped-auto",
            "office_spreadsheet",
            "executing",
            "auto",
            1,
        ))
        .unwrap();

    save_assistant_conversation_in_scope(
        &service,
        "conversation-unscoped-manual",
        "assistant-unscoped-manual",
        None,
    );
    let manual_call_id = "manual-call";
    let manual_storage_id = current_pending_storage_id("run-unscoped-manual", manual_call_id);
    let trace = seed_current_in_progress_tool_trace(
        &service,
        "run-unscoped-manual",
        "conversation-unscoped-manual",
        "assistant-unscoped-manual",
        manual_call_id,
        "skills_run_script",
    );
    let mut pending = interrupted_file_effect(
        manual_call_id,
        "run-unscoped-manual",
        "conversation-unscoped-manual",
        "assistant-unscoped-manual",
        "skills_run_script",
        2,
    );
    attach_current_manual_file_effect_checkpoint(&mut pending, &trace);
    service.store_pending_agent_action(pending).unwrap();
    service
        .upsert_agent_action_audit(file_effect_audit(
            &manual_storage_id,
            "run-unscoped-manual",
            "conversation-unscoped-manual",
            "skills_run_script",
            "approved",
            "manual",
            2,
        ))
        .unwrap();

    let interrupted = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(interrupted.len(), 1);

    let mut restored = service.list_unsettled_file_effects().unwrap();
    restored.sort_by(|left, right| left.action_id.cmp(&right.action_id));
    assert_eq!(
        restored,
        vec![
            AgentUnsettledFileEffect {
                project_id: None,
                conversation_id: "conversation-unscoped-auto".to_string(),
                run_id: "run-unscoped-auto".to_string(),
                action_id: auto_storage_id,
            },
            AgentUnsettledFileEffect {
                project_id: None,
                conversation_id: "conversation-unscoped-manual".to_string(),
                run_id: "run-unscoped-manual".to_string(),
                action_id: manual_storage_id,
            },
        ]
    );
}

#[test]
fn startup_reconciliation_excludes_authoritatively_settled_manual_file_effects() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (mut command_pending, command_approved, command_terminal, command_trace) =
        manual_command_settlement(
            "settled-command",
            "conversation-settled-command",
            "assistant-settled-command",
        );
    attach_current_manual_file_effect_checkpoint(&mut command_pending, &command_trace);
    save_assistant_conversation(
        &service,
        "conversation-settled-command",
        "assistant-settled-command",
    );
    service.store_pending_agent_action(command_pending).unwrap();
    service.upsert_agent_action_audit(command_approved).unwrap();
    let exact_model_items = vec![
        crate::ConversationModelContextItem {
            images: Vec::new(),
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: "settled-command".to_string(),
                name: "run_command".to_string(),
                args: match &command_trace.items[0] {
                    ConversationTurnTraceItem::ToolCall { operation, .. } => operation.clone(),
                    _ => panic!("manual settlement must start with a ToolCall"),
                },
                provider_identity: crate::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: "settled-command".to_string(),
                    runtime_call_id: "settled-command".to_string(),
                },
            }],
            is_error: false,
        },
        crate::ConversationModelContextItem {
            images: Vec::new(),
            sequence: 1,
            ordinal: 0,
            role: "tool".to_string(),
            content: r#"{"ok":true,"result":{"stdout":"EXACT_APPROVAL_RESULT"}}"#.to_string(),
            tool_call_id: Some("settled-command".to_string()),
            tool_calls: Vec::new(),
            is_error: false,
        },
    ];
    service
        .commit_pending_agent_action_audited_result_trace_with_model_context(
            &command_terminal,
            "approved",
            "completed",
            &command_trace,
            &exact_model_items,
            12,
        )
        .unwrap();
    assert_eq!(
        service
            .get_conversation_model_context_log("assistant-settled-command")
            .unwrap()
            .unwrap()
            .items,
        exact_model_items
    );

    let mut expected_action_ids = vec![current_pending_storage_id("run-1", "settled-command")];
    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let call_id = format!("settled-{label}-call");
        let run_id = format!("run-settled-{label}");
        let storage_id = current_pending_storage_id(&run_id, &call_id);
        let conversation_id = format!("conversation-settled-{label}");
        let assistant_message_id = format!("assistant-settled-{label}");
        let (mut pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        attach_current_manual_file_effect_checkpoint(&mut pending, &trace);
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
            .unwrap();
        expected_action_ids.push(storage_id);
    }

    assert_eq!(
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap()
            .len(),
        expected_action_ids.len()
    );
    assert!(service.list_unsettled_file_effects().unwrap().is_empty());
    let connection = service.state.connection().unwrap();
    for action_id in expected_action_ids {
        let state: (String, Option<String>) = connection
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&action_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            ("completed".to_string(), Some("completed".to_string()))
        );
    }
    drop(connection);
    let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    assert!(reopened.list_unsettled_file_effects().unwrap().is_empty());
    assert!(reopened
        .get_conversation_model_context_log("assistant-settled-command")
        .unwrap()
        .unwrap()
        .items[1]
        .content
        .contains("EXACT_APPROVAL_RESULT"));
}
