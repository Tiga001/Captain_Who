use super::command_fixture::manual_command_settlement;
use super::fixtures::*;
use super::*;

#[test]
fn manual_command_audit_target_and_trace_commit_as_one_idempotent_transaction() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let storage_id = current_pending_storage_id("run-1", "manual-command");
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "manual-command",
        "conversation-manual-command",
        "assistant-manual-command",
    );
    save_assistant_conversation(
        &service,
        "conversation-manual-command",
        "assistant-manual-command",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();

    let outcome = service
        .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
        .unwrap();
    assert_eq!(
        outcome,
        AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true
        }
    );

    let connection = service.state.connection().unwrap();
    let (target_status, audit_status, decided_at, completed_at): (
        Option<String>,
        String,
        Option<i64>,
        Option<i64>,
    ) = connection
        .query_row(
            "
            SELECT pending.target_status, audit.status, audit.decided_at, audit.completed_at
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(target_status.as_deref(), Some("completed"));
    assert_eq!(audit_status, "completed");
    assert_eq!(decided_at, Some(11), "approval timestamp must be preserved");
    assert_eq!(completed_at, Some(12));
    drop(connection);

    let retry = service
        .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
        .unwrap();
    assert_eq!(retry, AgentPendingActionResultCommitOutcome::Idempotent);
}

#[test]
fn failed_managed_pdf_settlement_preserves_opaque_history_route_across_durable_audit() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (mut pending, mut approved, mut terminal, mut trace) = manual_command_settlement(
        "managed-pdf-failure",
        "conversation-managed-pdf-failure",
        "assistant-managed-pdf-failure",
    );
    let command = "pdftotext \"$MYCOPILOT_INPUT_ROOT/missing.pdf\" -";
    let mut frozen_action =
        serde_json::from_str::<AgentProposedAction>(&pending.action_json).unwrap();
    let AgentProposedAction::Command {
        command: frozen_command,
    } = &mut frozen_action
    else {
        unreachable!("manual command settlement must freeze a command action");
    };
    frozen_command.command = command.to_string();
    let frozen_action_json = serde_json::to_string(&frozen_action).unwrap();
    pending.action_json = frozen_action_json.clone();
    approved.action_json = frozen_action_json.clone();
    terminal.action_json = frozen_action_json;

    let mut command_result = serde_json::from_str::<AgentCommandExecutionResult>(
        terminal.command_result_json.as_deref().unwrap(),
    )
    .unwrap();
    command_result.command = command.to_string();
    command_result.exit_code = Some(1);
    command_result.stdout.clear();
    command_result.stderr = "missing.pdf: No such file or directory".to_string();
    command_result.runtime = Some(crate::AgentCommandRuntimeResolution {
        schema_version: crate::AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
        provider_id: crate::artifact_runtime::ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
        profile: Some(crate::AgentCommandRuntimeProfile::Pdf),
        profile_revision: Some("pdf-profile-v1".to_string()),
        bundle_version: Some("test-bundle".to_string()),
        bundle_revision: Some("test-bundle-revision".to_string()),
        kind: crate::AgentCommandRuntimeKind::Python,
        runtime_version: Some("3.12.0".to_string()),
        runtime_fingerprint: Some("artifact-runtime-sha256-v1:test".to_string()),
        resolved_packages: Vec::new(),
        error_code: None,
        recovery: None,
        message: None,
    });
    crate::command::bind_authoritative_command_archive(
        &mut command_result,
        "archive-managed-pdf-failure".to_string(),
    )
    .unwrap();

    let live_tool_result =
        crate::command::command_tool_result("managed-pdf-failure", &command_result);
    assert!(!live_tool_result.ok);
    let history_open = live_tool_result.result.as_ref().unwrap()["historyOpen"]
        .as_str()
        .unwrap()
        .to_string();
    let durable_command_json = serde_json::to_string(&command_result).unwrap();
    assert!(durable_command_json.contains("\"historyOpen\""));
    assert!(!durable_command_json.contains("authoritativeArchiveRef"));
    let durable_command =
        serde_json::from_str::<AgentCommandExecutionResult>(&durable_command_json).unwrap();
    assert!(durable_command.authoritative_archive_ref.is_none());
    assert_eq!(
        durable_command.history_open.as_deref(),
        Some(history_open.as_str())
    );
    let rebuilt_tool_result =
        crate::command::command_tool_result("managed-pdf-failure", &durable_command);
    assert_eq!(rebuilt_tool_result.ok, live_tool_result.ok);
    assert_eq!(rebuilt_tool_result.error, live_tool_result.error);
    assert_eq!(rebuilt_tool_result.result, live_tool_result.result);

    terminal.status = "failed".to_string();
    terminal.command_result_json = Some(durable_command_json);
    terminal.tool_result_json = Some(serde_json::to_string(&live_tool_result).unwrap());
    terminal.error = live_tool_result.error.clone();
    let trace_operation = serde_json::json!({
        "command": command,
        "reason": "test atomic settlement",
    });
    let ConversationTurnTraceItem::ToolCall { operation, .. } = &mut trace.items[0] else {
        unreachable!("manual command settlement trace must start with a ToolCall");
    };
    *operation = trace_operation.clone();
    let trace_call = AgentToolCall {
        id: "managed-pdf-failure".to_string(),
        tool: "run_command".to_string(),
        args: trace_operation,
        approval_status: crate::AgentApprovalStatus::Approved,
        reason: Some("test atomic settlement".to_string()),
    };
    trace.items[1] = crate::conversation_trace::projected_tool_result_trace_item(
        1,
        &trace_call,
        &live_tool_result,
    );

    save_assistant_conversation(
        &service,
        "conversation-managed-pdf-failure",
        "assistant-managed-pdf-failure",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();
    assert_eq!(
        service
            .commit_current_manual_settlement(&terminal, "approved", "failed", &trace, 12,)
            .unwrap(),
        AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true,
        }
    );
    assert!(service.list_unsettled_file_effects().unwrap().is_empty());

    let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    assert_eq!(
        reopened
            .commit_current_manual_settlement(&terminal, "approved", "failed", &trace, 12,)
            .unwrap(),
        AgentPendingActionResultCommitOutcome::Idempotent
    );
    assert_eq!(
        reopened
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap()
            .len(),
        1
    );
    assert!(reopened.list_unsettled_file_effects().unwrap().is_empty());
    assert!(reopened
        .reconcile_interrupted_pending_agent_actions(43)
        .unwrap()
        .is_empty());
}

#[test]
fn manual_command_audit_failure_wrapper_preserves_the_exact_execution_evidence() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, approved, mut terminal, mut trace) = manual_command_settlement(
        "audit-wrapper-command",
        "conversation-audit-wrapper-command",
        "assistant-audit-wrapper-command",
    );
    save_assistant_conversation(
        &service,
        "conversation-audit-wrapper-command",
        "assistant-audit-wrapper-command",
    );
    let command_result = serde_json::from_str::<AgentCommandExecutionResult>(
        terminal.command_result_json.as_deref().unwrap(),
    )
    .unwrap();
    let message = "The command finished, but its final action audit could not be persisted. Inspect the observed artifacts before retrying.";
    let canonical_execution =
        crate::command::command_tool_result("audit-wrapper-command", &command_result)
            .result
            .unwrap();
    let fallback = AgentToolResult {
        exact_archive_file: None,
        call_id: "audit-wrapper-command".to_string(),
        tool: "run_command".to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "command_execution",
            "code": "auditPersistenceFailed",
            "recovery": "inspectArtifacts",
            "phase": "afterExecution",
            "executionAttempted": true,
            "effectsMayHaveOccurred": true,
            "auditError": "simulated first-commit failure",
            "execution": canonical_execution,
        })),
        error: Some(message.to_string()),
    };
    terminal.status = "failed".to_string();
    terminal.tool_result_json = Some(serde_json::to_string(&fallback).unwrap());
    terminal.error = fallback.error.clone();
    let call = AgentToolCall {
        id: "audit-wrapper-command".to_string(),
        tool: "run_command".to_string(),
        args: serde_json::json!({
            "command": "node script.mjs",
            "reason": "test atomic settlement",
        }),
        approval_status: crate::AgentApprovalStatus::Approved,
        reason: Some("test atomic settlement".to_string()),
    };
    trace.items[1] =
        crate::conversation_trace::projected_tool_result_trace_item(1, &call, &fallback);

    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();
    service
        .commit_current_manual_settlement(&terminal, "approved", "failed", &trace, 12)
        .unwrap();
    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert!(service.list_unsettled_file_effects().unwrap().is_empty());
}

#[test]
fn manual_settlement_short_prefix_allows_only_monotonic_truncation() {
    for (case, durable_truncated, expected_truncated, change_committed_call, uncommitted) in [
        ("new bounded result", false, true, false, true),
        ("truncation removed", true, false, false, false),
        ("committed call changed", false, true, true, false),
    ] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let (pending, approved, terminal, mut expected) = manual_command_settlement(
            "inspect-truncated-command",
            "conversation-inspect-truncated",
            "assistant-inspect-truncated",
        );
        save_assistant_conversation(
            &service,
            "conversation-inspect-truncated",
            "assistant-inspect-truncated",
        );
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        expected.truncated = expected_truncated;
        let mut durable = expected.clone();
        durable.items.truncate(1);
        durable.truncated = durable_truncated;
        if change_committed_call {
            let ConversationTurnTraceItem::ToolCall { operation, .. } = &mut durable.items[0]
            else {
                panic!("fixture must begin with its admitted ToolCall");
            };
            operation["command"] = serde_json::json!("a different command");
        }
        service
            .append_in_progress_conversation_turn_trace(&durable, 10, 11)
            .unwrap();

        let inspection = service
            .inspect_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &expected,
                12,
            )
            .unwrap();
        if uncommitted {
            assert_eq!(
                inspection,
                AgentPendingActionSettlementInspection::DefinitelyUncommitted,
                "{case}"
            );
        } else {
            assert!(
                matches!(
                    inspection,
                    AgentPendingActionSettlementInspection::Diverged {
                        component: "conversationTrace",
                        ..
                    }
                ),
                "{case}: {inspection:?}"
            );
        }
    }
}

#[test]
fn manual_command_settlement_inspection_distinguishes_commit_boundaries() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "inspect-command",
        "conversation-inspect-command",
        "assistant-inspect-command",
    );
    save_assistant_conversation(
        &service,
        "conversation-inspect-command",
        "assistant-inspect-command",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();

    assert_eq!(
        service
            .inspect_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap(),
        AgentPendingActionSettlementInspection::DefinitelyUncommitted
    );

    service
        .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
        .unwrap();
    assert_eq!(
        service
            .inspect_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap(),
        AgentPendingActionSettlementInspection::CommittedAtBoundary
    );

    let mut advanced = trace.clone();
    advanced
        .items
        .push(ConversationTurnTraceItem::AssistantNarration {
            provider_turn_id: None,
            first_tool_call_id: None,
            sequence: 2,
            content: "continued".to_string(),
            truncated: false,
        });
    service
        .append_in_progress_conversation_turn_trace(&advanced, 10, 13)
        .unwrap();
    assert_eq!(
        service
            .inspect_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap(),
        AgentPendingActionSettlementInspection::CommittedAndAdvanced
    );

    let mut conflicting_terminal = terminal;
    conflicting_terminal.command_result_json = conflicting_terminal
        .command_result_json
        .map(|json| json.replace("created workbook", "different output"));
    let error = service
        .inspect_pending_agent_action_audited_result_trace(
            &conflicting_terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap_err();
    assert!(error.contains("durable command execution result"));
}

#[test]
fn manual_command_trace_failure_rolls_back_audit_and_target() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let storage_id = current_pending_storage_id("run-1", "rollback-command");
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "rollback-command",
        "conversation-without-message",
        "assistant-without-message",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();

    let error = service
        .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
        .unwrap_err();
    assert!(error.contains("assistant message does not exist"));

    let connection = service.state.connection().unwrap();
    let (target_status, audit_status): (Option<String>, String) = connection
        .query_row(
            "
            SELECT pending.target_status, audit.status
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(target_status, None);
    assert_eq!(audit_status, "approved");
}

#[test]
fn manual_command_terminal_retry_with_different_result_conflicts_without_overwrite() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "conflict-command",
        "conversation-conflict-command",
        "assistant-conflict-command",
    );
    save_assistant_conversation(
        &service,
        "conversation-conflict-command",
        "assistant-conflict-command",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();
    service
        .commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
        .unwrap();

    let mut conflicting = terminal.clone();
    let mut command_result = serde_json::from_str::<AgentCommandExecutionResult>(
        conflicting.command_result_json.as_deref().unwrap(),
    )
    .unwrap();
    command_result.stdout = "different output".to_string();
    conflicting.command_result_json = Some(serde_json::to_string(&command_result).unwrap());
    let error = service
        .commit_current_manual_settlement(&conflicting, "approved", "completed", &trace, 12)
        .unwrap_err();
    assert!(error.contains("durable command execution result"));

    let persisted = service.list_agent_command_results_for_run("run-1").unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].stdout, "created workbook");
}

#[test]
fn concurrent_manual_command_settlement_is_exactly_once_across_storage_instances() {
    let fixture = StorageFixture::new();
    let first = fixture.service();
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "concurrent-command",
        "conversation-concurrent-command",
        "assistant-concurrent-command",
    );
    save_assistant_conversation(
        &first,
        "conversation-concurrent-command",
        "assistant-concurrent-command",
    );
    first.store_pending_agent_action(pending).unwrap();
    first.upsert_agent_action_audit(approved).unwrap();
    let second = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

    let first_barrier = std::sync::Arc::clone(&barrier);
    let first_terminal = terminal.clone();
    let first_trace = trace.clone();
    let first_thread = std::thread::spawn(move || {
        first_barrier.wait();
        first.commit_current_manual_settlement(
            &first_terminal,
            "approved",
            "completed",
            &first_trace,
            12,
        )
    });
    let second_barrier = std::sync::Arc::clone(&barrier);
    let second_thread = std::thread::spawn(move || {
        second_barrier.wait();
        second.commit_current_manual_settlement(&terminal, "approved", "completed", &trace, 12)
    });

    let outcomes = [
        first_thread.join().unwrap().unwrap(),
        second_thread.join().unwrap().unwrap(),
    ];
    assert!(
        outcomes.contains(&AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true,
        })
    );
    assert!(outcomes.contains(&AgentPendingActionResultCommitOutcome::Idempotent));
}
