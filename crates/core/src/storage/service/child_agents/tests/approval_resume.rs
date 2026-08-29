use super::*;
use crate::storage::models::{
    AgentActionAuditRecord, AgentPendingActionRecord, AgentUsageRecordInsert,
};
use rusqlite::params;

fn waiting_skill_run(action_id: &str) -> String {
    serde_json::json!({
        "runId": "approval-run",
        "status": "waiting_for_approval",
        "startedAt": 20,
        "toolDefinitions": [],
        "toolCalls": [{
            "id": action_id,
            "tool": "skills_run_script",
            "args": { "scriptUri": "skill://fixture/scripts/check.py" },
            "approvalStatus": "required",
            "reason": null
        }],
        "toolResults": [],
        "approvals": [{
            "type": "skill_script",
            "script": {
                "id": action_id,
                "scriptUri": "skill://fixture/scripts/check.py",
                "skillId": "installed:user:fixture",
                "skillRevision": "revision-1",
                "resourcePath": "scripts/check.py",
                "resourceDigest": "sha256:fixture",
                "source": {
                    "sourceId": "installed:user",
                    "sourceKind": "installed",
                    "trust": "untrusted"
                },
                "interpreter": "python3",
                "args": [],
                "requirements": {
                    "pythonDistributions": [],
                    "commands": []
                },
                "preflight": {
                    "status": "ready",
                    "interpreter": "python3",
                    "interpreterVersion": "Python 3",
                    "dependencies": [],
                    "runtimeFingerprint": "fixture-runtime"
                },
                "timeoutMs": null,
                "approvalStatus": "required",
                "reason": null
            }
        }],
        "fileChangeProposals": [],
        "fileChanges": [],
        "webSearchActivities": [],
        "readActivities": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {},
        "timeline": [{
            "id": "approval-fixture-message",
            "type": "message",
            "content": "waiting timeline must be preserved"
        }],
        "state": {
            "status": "waiting_for_approval",
            "activeRunId": "approval-run",
            "lastError": null,
            "updatedAt": 20
        }
    })
    .to_string()
}

fn pending_action(action_id: &str, conversation_id: &str) -> AgentPendingActionRecord {
    AgentPendingActionRecord {
        action_id: action_id.to_string(),
        run_id: "approval-run".to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some("approval-assistant".to_string()),
        action_type: "skill_script".to_string(),
        tool_name: "skills_run_script".to_string(),
        tool_call_id: Some(action_id.to_string()),
        status: "pending".to_string(),
        target_status: None,
        action_json: "{}".to_string(),
        agent_input_json: "{}".to_string(),
        created_at: 20,
        updated_at: 20,
    }
}

fn audit(
    action: &AgentPendingActionRecord,
    decision: Option<&str>,
    status: &str,
    decided_at: Option<i64>,
) -> AgentActionAuditRecord {
    AgentActionAuditRecord {
        action_id: action.action_id.clone(),
        run_id: action.run_id.clone(),
        conversation_id: action.conversation_id.clone(),
        assistant_message_id: action.assistant_message_id.clone(),
        action_type: action.action_type.clone(),
        tool_name: action.tool_name.clone(),
        decision: decision.map(str::to_string),
        status: status.to_string(),
        action_json: action.action_json.clone(),
        file_change_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: action.created_at,
        decided_at,
        completed_at: None,
        effective_permissions_json: Some("{}".to_string()),
        path_scope: None,
        command_cwd_scope: None,
        blocked_reason: None,
        decision_source: Some(
            if decision.is_some() {
                "manual"
            } else {
                "manual_pending"
            }
            .to_string(),
        ),
    }
}

fn active_waiting_child(fixture: &Fixture, action_id: &str) -> (String, String, String, u64) {
    let child = fixture
        .service
        .create_child_agent(&spawn_input("approval-spawn", "approval_child"))
        .unwrap();
    let claim_token = "approval-claim";
    let claimed = fixture
        .service
        .claim_next_agent_wake(&child.agent.agent_id, claim_token)
        .unwrap()
        .unwrap();
    fixture
        .service
        .transition_agent_wake(
            &claimed.wake_id,
            AgentWakeStatus::Claimed,
            AgentWakeStatus::Running,
            Some(claim_token),
        )
        .unwrap();

    {
        let mut connection = fixture.service.state.connection().unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, agent_run_json,
                     created_at, position
                 ) VALUES (
                     'approval-assistant', ?1, 'assistant', '', 'pending', ?2, 20,
                     (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                      WHERE conversation_id = ?1)
                 )",
                params![&child.agent.conversation_id, waiting_skill_run(action_id)],
            )
            .unwrap();
        let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
            "approval-run",
            &child.agent.conversation_id,
            "approval-assistant",
        );
        crate::storage::conversation_trace_repository::append_in_progress_trace(
            &mut connection,
            &trace,
            20,
            20,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE agent_wake_requests
                 SET run_id = 'approval-run', assistant_message_id = 'approval-assistant'
                 WHERE wake_id = ?1",
                [&claimed.wake_id],
            )
            .unwrap();
    }
    fixture
        .service
        .upsert_agent_usage(AgentUsageRecordInsert {
            id: format!("usage-{action_id}"),
            conversation_id: child.agent.conversation_id.clone(),
            message_id: "approval-assistant".to_string(),
            run_id: "approval-run".to_string(),
            project_id: None,
            model_id: "model-a".to_string(),
            model_name: "Model A".to_string(),
            started_at: Some(20),
            completed_at: None,
            status: Some("waiting_for_approval".to_string()),
            error: None,
            created_at: 20,
            input_tokens: None,
            output_tokens: None,
            output_thinking_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: 0,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            estimated_cost: None,
        })
        .unwrap();
    let waiting = fixture
        .service
        .transition_agent_wake(
            &claimed.wake_id,
            AgentWakeStatus::Running,
            AgentWakeStatus::WaitingForApproval,
            Some(claim_token),
        )
        .unwrap();
    (
        claimed.wake_id,
        claim_token.to_string(),
        child.agent.conversation_id,
        waiting.status_revision,
    )
}

#[test]
fn approval_execution_atomically_claims_action_audit_and_child_wake() {
    let fixture = Fixture::new(Some("model-a"));
    let action_id = "approval-action";
    let (wake_id, claim_token, conversation_id, waiting_revision) =
        active_waiting_child(&fixture, action_id);
    let child = fixture.service.get_agent_wake(&wake_id).unwrap().unwrap();
    let action = pending_action(action_id, &conversation_id);
    assert_eq!(
        child.status,
        AgentWakeStatus::WaitingForApproval,
        "fixture must start at the broken-state boundary"
    );
    fixture
        .service
        .store_pending_agent_action(action.clone())
        .unwrap();
    fixture
        .service
        .upsert_agent_action_audit(audit(&action, None, "pending", None))
        .unwrap();

    fixture
        .service
        .commit_pending_skill_script_approval_execution(
            &action.action_id,
            "{}",
            &audit(&action, Some("approved"), "approved", Some(30)),
            Some((&wake_id, AgentWakeStatus::WaitingForApproval, &claim_token)),
            30,
        )
        .unwrap();

    let resumed = fixture.service.get_agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(resumed.status, AgentWakeStatus::Running);
    assert_eq!(resumed.status_revision, waiting_revision + 1);
    let connection = fixture.service.state.connection().unwrap();
    let (pending_status, audit_decision, audit_status): (String, Option<String>, String) =
        connection
            .query_row(
                "SELECT pending.status, audit.decision, audit.status
                 FROM agent_pending_actions AS pending
                 JOIN agent_action_audit AS audit ON audit.action_id = pending.action_id
                 WHERE pending.action_id = ?1",
                [&action.action_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
    assert_eq!(pending_status, "executing");
    assert_eq!(audit_decision.as_deref(), Some("approved"));
    assert_eq!(audit_status, "approved");
    let (message_status, agent_run_json, usage_status): (String, String, String) = connection
        .query_row(
            "SELECT message.status, message.agent_run_json, usage.status
             FROM messages AS message
             JOIN agent_usage_records AS usage
               ON usage.conversation_id = message.conversation_id
              AND usage.message_id = message.id
             WHERE message.conversation_id = ?1 AND message.id = 'approval-assistant'",
            [&conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(message_status, "pending");
    assert_eq!(usage_status, "running");
    let run: serde_json::Value = serde_json::from_str(&agent_run_json).unwrap();
    assert_eq!(run["status"], "running");
    assert_eq!(run["state"]["status"], "running");
    assert_eq!(run["state"]["activeRunId"], "approval-run");
    assert_eq!(run["toolCalls"][0]["approvalStatus"], "approved");
    assert_eq!(run["approvals"][0]["script"]["approvalStatus"], "approved");
    assert_eq!(
        run["timeline"][0]["content"],
        "waiting timeline must be preserved"
    );
}

#[test]
fn approval_execution_projection_failure_rolls_back_every_fact() {
    let fixture = Fixture::new(Some("model-a"));
    let action_id = "rollback-action";
    let (wake_id, claim_token, conversation_id, _) = active_waiting_child(&fixture, action_id);
    let action = pending_action(action_id, &conversation_id);
    fixture
        .service
        .store_pending_agent_action(action.clone())
        .unwrap();
    fixture
        .service
        .upsert_agent_action_audit(audit(&action, None, "pending", None))
        .unwrap();
    {
        let connection = fixture.service.state.connection().unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TRIGGER fail_approval_usage_resume
                 BEFORE UPDATE OF status ON agent_usage_records
                 WHEN OLD.status = 'waiting_for_approval' AND NEW.status = 'running'
                 BEGIN SELECT RAISE(ABORT, 'injected approval Usage failure'); END;",
            )
            .unwrap();
    }

    assert!(fixture
        .service
        .commit_pending_skill_script_approval_execution(
            &action.action_id,
            "{}",
            &audit(&action, Some("approved"), "approved", Some(30)),
            Some((&wake_id, AgentWakeStatus::WaitingForApproval, &claim_token,)),
            30,
        )
        .is_err());

    let wake = fixture.service.get_agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(wake.status, AgentWakeStatus::WaitingForApproval);
    let connection = fixture.service.state.connection().unwrap();
    let (pending_status, audit_decision, audit_status): (String, Option<String>, String) =
        connection
            .query_row(
                "SELECT pending.status, audit.decision, audit.status
                 FROM agent_pending_actions AS pending
                 JOIN agent_action_audit AS audit ON audit.action_id = pending.action_id
                 WHERE pending.action_id = ?1",
                params![&action.action_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
    assert_eq!(pending_status, "pending");
    assert_eq!(audit_decision, None);
    assert_eq!(audit_status, "pending");
    let (agent_run_json, usage_status): (String, String) = connection
        .query_row(
            "SELECT message.agent_run_json, usage.status
             FROM messages AS message
             JOIN agent_usage_records AS usage
               ON usage.conversation_id = message.conversation_id
              AND usage.message_id = message.id
             WHERE message.conversation_id = ?1 AND message.id = 'approval-assistant'",
            [&conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let run: serde_json::Value = serde_json::from_str(&agent_run_json).unwrap();
    assert_eq!(run["status"], "waiting_for_approval");
    assert_eq!(run["state"]["status"], "waiting_for_approval");
    assert_eq!(run["toolCalls"][0]["approvalStatus"], "required");
    assert_eq!(run["approvals"][0]["script"]["approvalStatus"], "required");
    assert_eq!(usage_status, "waiting_for_approval");
}

#[test]
fn stale_dispatcher_wait_observation_cannot_reverse_an_approved_wake() {
    let fixture = Fixture::new(Some("model-a"));
    let action_id = "approval-dispatch-race-action";
    let (wake_id, claim_token, conversation_id, _) = active_waiting_child(&fixture, action_id);
    let running_before_approval = fixture
        .service
        .transition_agent_wake(
            &wake_id,
            AgentWakeStatus::WaitingForApproval,
            AgentWakeStatus::Running,
            Some(&claim_token),
        )
        .unwrap();
    let action = pending_action(action_id, &conversation_id);
    fixture
        .service
        .store_pending_agent_action(action.clone())
        .unwrap();
    fixture
        .service
        .upsert_agent_action_audit(audit(&action, None, "pending", None))
        .unwrap();

    let Fixture {
        _directory,
        service,
    } = fixture;
    let service = std::sync::Arc::new(service);
    let inspected = std::sync::Arc::new(std::sync::Barrier::new(2));
    let approval_committed = std::sync::Arc::new(std::sync::Barrier::new(2));
    let dispatcher = {
        let service = std::sync::Arc::clone(&service);
        let inspected = std::sync::Arc::clone(&inspected);
        let approval_committed = std::sync::Arc::clone(&approval_committed);
        let wake_id = wake_id.clone();
        let claim_token = claim_token.clone();
        let conversation_id = conversation_id.clone();
        std::thread::spawn(move || {
            assert!(service
                .list_pending_agent_actions()
                .unwrap()
                .iter()
                .any(|pending| pending.action_id == action_id && pending.status == "pending"));
            inspected.wait();
            approval_committed.wait();
            service
                .mark_agent_wake_waiting_for_pending_approval_at(
                    &wake_id,
                    "approval-run",
                    &conversation_id,
                    "approval-assistant",
                    &claim_token,
                    31,
                )
                .unwrap()
        })
    };

    inspected.wait();
    service
        .commit_pending_skill_script_approval_execution(
            &action.action_id,
            "{}",
            &audit(&action, Some("approved"), "approved", Some(30)),
            Some((&wake_id, AgentWakeStatus::Running, &claim_token)),
            30,
        )
        .unwrap();
    approval_committed.wait();

    let outcome = dispatcher.join().unwrap();
    let AgentWakeApprovalWaitOutcome::RunningAfterApproval(wake) = outcome else {
        panic!("a stale dispatcher observation must become a running no-op");
    };
    assert_eq!(wake.status, AgentWakeStatus::Running);
    assert_eq!(
        wake.status_revision, running_before_approval.status_revision,
        "neither the idempotent approval resume nor the stale dispatcher may rewrite the Wake"
    );
    assert_eq!(
        service
            .get_pending_agent_action(&action.action_id)
            .unwrap()
            .unwrap()
            .status,
        "executing"
    );
    drop(_directory);
}
