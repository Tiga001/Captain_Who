use super::fixtures::*;
use super::non_command_fixture::{
    manual_non_command_file_effect_settlement, ManualNonCommandFileEffect,
};
use super::*;

type CommittedManualFileEffectState = (
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
);

fn assert_manual_file_effect_is_uncommitted(
    service: &StorageService,
    storage_id: &str,
    assistant_message_id: &str,
    frozen_action_json: &str,
) {
    let connection = service.state.connection().unwrap();
    let state: (
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        String,
    ) = connection
        .query_row(
            "
            SELECT pending.target_status,
                   audit.status,
                   audit.command_result_json,
                   audit.tool_result_json,
                   audit.action_json
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            [storage_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(state.0, None, "pending target must remain uncommitted");
    assert_eq!(state.1, "approved", "audit must remain preterminal");
    assert_eq!(state.2, None, "non-command result column must stay empty");
    assert_eq!(state.3, None, "terminal ToolResult must not leak");
    assert_eq!(state.4, frozen_action_json, "frozen action changed");
    drop(connection);
    assert!(service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .is_none());
}

#[test]
fn every_manual_non_command_file_effect_settles_atomically_and_idempotently() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let call_id = format!("{label}-call");
        let run_id = format!("run-{label}");
        let storage_id = current_pending_storage_id(&run_id, &call_id);
        let conversation_id = format!("conversation-{label}");
        let assistant_message_id = format!("assistant-{label}");
        let (pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let frozen_action_json = pending.action_json.clone();
        let terminal_tool_result_json = terminal.tool_result_json.clone().unwrap();
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();

        let outcome = service
            .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
            .unwrap();
        assert_eq!(
            outcome,
            AgentPendingActionResultCommitOutcome::Committed {
                trace_changed: true
            },
            "{label} did not commit all durable records"
        );

        let connection = service.state.connection().unwrap();
        let state: CommittedManualFileEffectState = connection
            .query_row(
                "
                SELECT pending.target_status,
                       audit.status,
                       audit.command_result_json,
                       audit.tool_result_json,
                       audit.action_json,
                       pending.tool_name,
                       pending.tool_call_id
                FROM agent_pending_actions pending
                JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                WHERE pending.action_id = ?1
                ",
                [&storage_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(state.0.as_deref(), Some("completed"));
        assert_eq!(state.1, "completed");
        assert_eq!(
            state.2, None,
            "{label} must not populate command_result_json"
        );
        assert_eq!(state.3.as_deref(), Some(terminal_tool_result_json.as_str()));
        assert_eq!(state.4, frozen_action_json);
        assert_eq!(state.5, effect.tool_name());
        assert_eq!(state.6.as_deref(), Some(call_id.as_str()));
        drop(connection);
        assert_eq!(
            service
                .get_conversation_turn_trace(&assistant_message_id)
                .unwrap()
                .as_ref(),
            Some(&trace)
        );

        let retry = service
            .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
            .unwrap();
        assert_eq!(
            retry,
            AgentPendingActionResultCommitOutcome::Idempotent,
            "{label} exact retry was not idempotent"
        );
    }
}

#[test]
fn executing_skill_script_settles_with_its_terminal_tool_receipt() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let call_id = "executing-skill-call";
    let storage_id = current_pending_storage_id("run-executing-skill", call_id);
    let (mut pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
        ManualNonCommandFileEffect::SkillScript,
        call_id,
        "run-executing-skill",
        "conversation-executing-skill",
        "assistant-executing-skill",
    );
    pending.status = "executing".to_string();
    save_assistant_conversation(
        &service,
        "conversation-executing-skill",
        "assistant-executing-skill",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();

    let outcome = service
        .commit_current_manual_settlement(&terminal, "executing", "completed", &trace, 12)
        .unwrap();
    assert_eq!(
        outcome,
        AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true
        }
    );
    let settled = service
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(settled.status, "executing");
    assert_eq!(settled.target_status.as_deref(), Some("completed"));
}

#[test]
fn every_manual_non_command_trace_failure_rolls_back_audit_and_pending_target() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let call_id = format!("rollback-{label}-call");
        let run_id = format!("run-rollback-{label}");
        let storage_id = current_pending_storage_id(&run_id, &call_id);
        let conversation_id = format!("missing-conversation-{label}");
        let assistant_message_id = format!("missing-assistant-{label}");
        let (pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let frozen_action_json = pending.action_json.clone();
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();

        let error = service
            .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
            .unwrap_err();
        assert!(
            error.contains("assistant message does not exist"),
            "unexpected {label} trace failure: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );
    }
}

#[test]
fn manual_non_command_settlement_rejects_command_result_json_without_partial_state() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let call_id = format!("command-column-{label}-call");
        let run_id = format!("run-command-column-{label}");
        let storage_id = current_pending_storage_id(&run_id, &call_id);
        let conversation_id = format!("conversation-command-column-{label}");
        let assistant_message_id = format!("assistant-command-column-{label}");
        let (pending, approved, mut terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let frozen_action_json = pending.action_json.clone();
        terminal.command_result_json = Some("{}".to_string());
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();

        let error = service
            .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
            .unwrap_err();
        assert!(
            error.contains("non-command manual file-effect audit unexpectedly contains"),
            "unexpected {label} command-result validation error: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );
    }
}

#[test]
fn manual_non_command_settlement_strictly_binds_frozen_action_tool_and_call_identity() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let call_id = format!("identity-{label}-call");
        let run_id = format!("run-identity-{label}");
        let storage_id = current_pending_storage_id(&run_id, &call_id);
        let conversation_id = format!("conversation-identity-{label}");
        let assistant_message_id = format!("assistant-identity-{label}");
        let (pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let frozen_action_json = pending.action_json.clone();
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();

        let mut different_action = terminal.clone();
        different_action.action_json =
            serde_json::to_string(&effect.frozen_action(&call_id, true)).unwrap();
        let error = service
            .commit_current_manual_settlement(
                &different_action,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap_err();
        assert!(
            error.contains("does not match the frozen pending action"),
            "unexpected {label} frozen-action conflict: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );

        let mut different_tool = terminal.clone();
        different_tool.tool_name = "run_command".to_string();
        let error = service
            .commit_current_manual_settlement(&different_tool, "approved", "completed", &trace, 12)
            .unwrap_err();
        assert!(
            error.contains("audit type does not match the frozen action"),
            "unexpected {label} tool conflict: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );

        let different_call_id = format!("different-{call_id}");
        let mut different_call = terminal.clone();
        different_call.action_json =
            serde_json::to_string(&effect.frozen_action(&different_call_id, false)).unwrap();
        let mut different_call_tool_result = serde_json::from_str::<AgentToolResult>(
            different_call.tool_result_json.as_deref().unwrap(),
        )
        .unwrap();
        different_call_tool_result.call_id = different_call_id.clone();
        different_call.tool_result_json =
            Some(serde_json::to_string(&different_call_tool_result).unwrap());
        let mut different_call_trace = trace.clone();
        for item in &mut different_call_trace.items {
            match item {
                ConversationTurnTraceItem::ToolCall { call_id, .. }
                | ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                    *call_id = different_call_id.clone();
                }
                _ => {}
            }
        }
        let error = service
            .commit_current_manual_settlement(
                &different_call,
                "approved",
                "completed",
                &different_call_trace,
                12,
            )
            .unwrap_err();
        assert!(
            error.contains("does not match the frozen pending action"),
            "unexpected {label} call identity conflict: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );
    }
}

#[test]
fn manual_non_command_terminal_conflict_preserves_first_atomic_settlement() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let call_id = format!("terminal-conflict-{label}-call");
        let run_id = format!("run-terminal-conflict-{label}");
        let storage_id = current_pending_storage_id(&run_id, &call_id);
        let conversation_id = format!("conversation-terminal-conflict-{label}");
        let assistant_message_id = format!("assistant-terminal-conflict-{label}");
        let (pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let original_tool_result_json = terminal.tool_result_json.clone().unwrap();
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
            .unwrap();

        let mut conflicting = terminal.clone();
        let mut conflicting_tool_result = serde_json::from_str::<AgentToolResult>(
            conflicting.tool_result_json.as_deref().unwrap(),
        )
        .unwrap();
        conflicting_tool_result.result = Some(serde_json::json!({
            "status": "applied",
            "effect": label,
            "conflicting": true,
        }));
        conflicting.tool_result_json =
            Some(serde_json::to_string(&conflicting_tool_result).unwrap());
        let error = service
            .commit_current_manual_settlement(&conflicting, "approved", "completed", &trace, 12)
            .unwrap_err();
        assert!(
            error.contains("different terminal result"),
            "unexpected {label} terminal conflict: {error}"
        );

        let connection = service.state.connection().unwrap();
        let persisted: (Option<String>, String, Option<String>, Option<String>) = connection
            .query_row(
                "
                SELECT pending.target_status,
                       audit.status,
                       audit.command_result_json,
                       audit.tool_result_json
                FROM agent_pending_actions pending
                JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                WHERE pending.action_id = ?1
                ",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(persisted.0.as_deref(), Some("completed"));
        assert_eq!(persisted.1, "completed");
        assert_eq!(persisted.2, None);
        assert_eq!(
            persisted.3.as_deref(),
            Some(original_tool_result_json.as_str())
        );
        drop(connection);
        assert_eq!(
            service
                .get_conversation_turn_trace(&assistant_message_id)
                .unwrap()
                .as_ref(),
            Some(&trace)
        );
        assert_eq!(
            service
                .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12,)
                .unwrap(),
            AgentPendingActionResultCommitOutcome::Idempotent
        );
    }
}
