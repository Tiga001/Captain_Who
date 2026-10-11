use serde_json::{json, Map, Value};

/// Projects safe Skill installation history that cannot be rebuilt from ToolCall/Result alone:
/// the call retains only an installRef, while the approval owns the inspected package preview.
/// `safe_event` must have passed RendererSafeAgentEvent's privacy boundary.
pub fn project_checkpoint_activity(
    projection: &mut Map<String, Value>,
    safe_event: &Value,
    _now: i64,
) -> bool {
    match safe_event.get("type").and_then(Value::as_str) {
        Some("approval_required") => safe_event
            .get("action")
            .is_some_and(|action| project_installation_approval(projection, action)),
        Some("done") => {
            let mut changed = false;
            if let Some(actions) = safe_event.get("proposedActions").and_then(Value::as_array) {
                for action in actions {
                    changed |= project_installation_approval(projection, action);
                }
            }
            changed
        }
        Some("tool_result") => safe_event
            .get("result")
            .is_some_and(|result| project_installation_result(projection, result)),
        _ => false,
    }
}

fn project_installation_approval(projection: &mut Map<String, Value>, action: &Value) -> bool {
    if action.get("type").and_then(Value::as_str) != Some("skill_installation") {
        return false;
    }
    let Some(installation) = action.get("installation").filter(|value| value.is_object()) else {
        return false;
    };
    let Some(id) = installation.get("id").and_then(Value::as_str) else {
        return false;
    };
    let installations = projection
        .entry("skillInstallations")
        .or_insert_with(|| json!([]));
    let Some(installations) = installations.as_array_mut() else {
        return false;
    };
    if let Some(existing) = installations
        .iter_mut()
        .find(|candidate| candidate["action"]["id"].as_str() == Some(id))
    {
        // Repeated Done/approval snapshots cannot reopen a settled installation.
        let next = json!({
            "action": installation,
            "status": existing.get("status").cloned().unwrap_or(json!("waiting_for_approval")),
        });
        if *existing == next {
            return false;
        }
        *existing = next;
    } else {
        installations.push(json!({
            "action": installation,
            "status": "waiting_for_approval",
        }));
    }
    true
}

fn project_installation_result(projection: &mut Map<String, Value>, result: &Value) -> bool {
    if result.get("tool").and_then(Value::as_str) != Some("skills_commit_install") {
        return false;
    }
    let Some(call_id) = result.get("callId").and_then(Value::as_str) else {
        return false;
    };
    let Some(installations) = projection
        .get_mut("skillInstallations")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    let Some(existing) = installations
        .iter_mut()
        .find(|candidate| candidate["action"]["id"].as_str() == Some(call_id))
        .and_then(Value::as_object_mut)
    else {
        // The result contains no inspected preview. Do not invent an installation history row.
        return false;
    };
    let details = result.get("result");
    let result_status = details
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str);
    let status = if result_status == Some("rejected") {
        // Rejection is emitted as a successful non-executing ToolResult.
        "rejected"
    } else if result.get("ok").and_then(Value::as_bool) != Some(true) {
        let uncertain = details.is_some_and(|value| {
            value.get("commitMayHaveSucceeded").and_then(Value::as_bool) == Some(true)
                || value
                    .get("code")
                    .and_then(Value::as_str)
                    .is_some_and(|code| code.to_ascii_lowercase().contains("uncertain"))
        });
        if uncertain {
            "uncertain"
        } else {
            "failed"
        }
    } else if result_status == Some("alreadyInstalled") {
        "already_installed"
    } else {
        "installed"
    };
    if existing.get("status").and_then(Value::as_str) == Some(status) {
        return false;
    }
    existing.insert("status".into(), json!(status));
    true
}

/// Rebuilds Skill installation display history from the same SQLite snapshot as its message and
/// Trace. These rows are presentation evidence only: reading never grants or replays execution.
pub(super) fn attach_skill_installation_history(
    connection: &rusqlite::Connection,
    conversation: &mut crate::storage::models::ChatConversationRecord,
) -> Result<(), String> {
    let messages = conversation
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == "assistant")
        .map(|(index, message)| (message.id.clone(), index))
        .collect::<std::collections::HashMap<_, _>>();
    if messages.is_empty() {
        return Ok(());
    }
    let ids = serde_json::to_string(&messages.keys().collect::<Vec<_>>())
        .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT pending.assistant_message_id, pending.run_id, pending.tool_call_id,
                    pending.action_json, pending.status, pending.updated_at,
                    audit.action_json, audit.tool_result_json
             FROM agent_pending_actions AS pending
             JOIN conversation_turn_traces AS trace
               ON trace.assistant_message_id = pending.assistant_message_id
              AND trace.conversation_id = pending.conversation_id
              AND trace.run_id = pending.run_id
             LEFT JOIN agent_action_audit AS audit
               ON audit.action_id = pending.action_id
              AND audit.conversation_id = pending.conversation_id
              AND audit.assistant_message_id = pending.assistant_message_id
              AND audit.run_id = pending.run_id
              AND audit.action_type = pending.action_type
              AND audit.tool_name = pending.tool_name
             WHERE pending.conversation_id = ?1
               AND pending.action_type = 'skill_installation'
               AND pending.tool_name = 'skills_commit_install'
               AND pending.assistant_message_id IN (SELECT value FROM json_each(?2))
             ORDER BY pending.created_at, pending.action_id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(rusqlite::params![conversation.id, ids], |row| {
            Ok(InstallationHistoryRow {
                message_id: row.get(0)?,
                run_id: row.get(1)?,
                tool_call_id: row.get(2)?,
                action_json: row.get(3)?,
                status: row.get(4)?,
                updated_at: row.get(5)?,
                audit_action_json: row.get(6)?,
                result_json: row.get(7)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    for row in rows {
        let Some(&index) = messages.get(&row.message_id) else {
            continue;
        };
        let message = &mut conversation.messages[index];
        let Some(raw) = message.agent_run_json.as_deref() else {
            continue;
        };
        let mut projection: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
        let Some(projection) = projection.as_object_mut() else {
            continue;
        };
        if projection.get("runId").and_then(Value::as_str) != Some(row.run_id.as_str()) {
            continue;
        }
        let crate::AgentProposedAction::SkillInstallation { installation } =
            serde_json::from_str(&row.action_json).map_err(|error| {
                format!("decode durable Skill installation approval projection: {error}")
            })?
        else {
            continue;
        };
        if row.tool_call_id.as_deref() != Some(installation.id.as_str()) {
            continue;
        }
        let action =
            serde_json::json!({ "type": "skill_installation", "installation": installation });
        project_installation_approval(projection, &action);
        let status = match row.status.as_str() {
            "pending" => "waiting_for_approval",
            "approved" | "executing" => "installing",
            "rejected" | "cancelled" => "rejected",
            "failed" => "failed",
            // Without a matching final receipt, completion alone cannot distinguish a new
            // installation, an already-installed package, or an uncertain filesystem outcome.
            "completed" => "uncertain",
            _ => continue,
        };
        let entry = projection
            .get_mut("skillInstallations")
            .and_then(Value::as_array_mut)
            .and_then(|entries| {
                entries
                    .iter_mut()
                    .find(|entry| entry["action"]["id"].as_str() == Some(installation.id.as_str()))
            });
        if let Some(entry) = entry {
            entry["status"] = json!(status);
        }
        if !matches!(row.status.as_str(), "pending" | "rejected" | "cancelled") {
            if let (Some(audit_json), Some(result_json)) =
                (row.audit_action_json.as_deref(), row.result_json.as_deref())
            {
                if let Ok(crate::AgentProposedAction::SkillInstallation {
                    installation: mut audit_installation,
                }) = serde_json::from_str(audit_json)
                {
                    audit_installation.approval_status = installation.approval_status;
                    if *audit_installation == *installation {
                        let result: Value = serde_json::from_str(result_json).map_err(|error| {
                            format!("decode Skill installation receipt: {error}")
                        })?;
                        if result.get("callId").and_then(Value::as_str)
                            == Some(installation.id.as_str())
                        {
                            project_checkpoint_activity(
                                projection,
                                &json!({ "type": "tool_result", "result": result }),
                                row.updated_at,
                            );
                        }
                    }
                }
            }
        }
        message.agent_run_json =
            Some(serde_json::to_string(projection).map_err(|error| error.to_string())?);
    }
    Ok(())
}

struct InstallationHistoryRow {
    message_id: String,
    run_id: String,
    tool_call_id: Option<String>,
    action_json: String,
    status: String,
    updated_at: i64,
    audit_action_json: Option<String>,
    result_json: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval() -> Value {
        json!({
            "type": "skill_installation",
            "installation": {
                "schemaVersion": 1,
                "id": "install-action",
                "installRef": "opaque-install-ref",
                "preview": {
                    "name": "Frozen package",
                    "description": "Fixture package",
                    "sourceSummary": {},
                    "resolvedRevision": "revision-1",
                    "fileCount": 1,
                    "totalBytes": 10,
                    "resourceSummary": { "total": 1, "references": 0, "assets": 0, "scripts": 0, "bytes": 10 },
                    "containsScripts": false,
                    "warnings": [],
                    "compatibility": "compatible",
                    "operation": "install",
                    "impact": "Adds one Skill",
                },
                "approvalStatus": "required",
                "expiresAt": 100,
            },
        })
    }

    fn projection() -> Map<String, Value> {
        let mut projection = Map::new();
        assert!(project_checkpoint_activity(
            &mut projection,
            &json!({ "type": "approval_required", "action": approval() }),
            1,
        ));
        projection
    }

    #[test]
    fn retains_safe_installation_preview_once_across_approval_and_done() {
        let mut projection = projection();
        let done = json!({ "type": "done", "proposedActions": [approval()] });
        assert!(!project_checkpoint_activity(&mut projection, &done, 2));
        assert_eq!(
            projection["skillInstallations"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            projection["skillInstallations"][0]["action"],
            approval()["installation"]
        );
        assert_eq!(
            projection["skillInstallations"][0]["status"],
            "waiting_for_approval"
        );
    }

    #[test]
    fn installation_result_preserves_exact_identity_and_does_not_reopen_on_done_replay() {
        for (ok, details, expected) in [
            (true, json!({"status": "installed"}), "installed"),
            (
                true,
                json!({"status": "alreadyInstalled"}),
                "already_installed",
            ),
            (true, json!({"status": "rejected"}), "rejected"),
            (false, json!({"code": "writeFailed"}), "failed"),
            (false, json!({"commitMayHaveSucceeded": true}), "uncertain"),
            (
                false,
                json!({"code": "commitOutcomeUNCERTAIN"}),
                "uncertain",
            ),
        ] {
            let mut projection = projection();
            let event = json!({
                "type": "tool_result",
                "result": {"callId": "install-action", "tool": "skills_commit_install", "ok": ok, "result": details},
            });
            assert!(project_checkpoint_activity(&mut projection, &event, 2));
            assert_eq!(projection["skillInstallations"][0]["status"], expected);
            assert!(!project_checkpoint_activity(&mut projection, &event, 3));
            assert!(!project_checkpoint_activity(
                &mut projection,
                &json!({ "type": "done", "proposedActions": [approval()] }),
                4,
            ));
            assert_eq!(projection["skillInstallations"][0]["status"], expected);
        }
    }

    #[test]
    fn ignores_unrelated_results_and_protected_approval_payloads() {
        let mut projection = projection();
        let before = projection.clone();
        for event in [
            json!({"type": "tool_result", "result": {"callId": "other", "tool": "skills_commit_install", "ok": true}}),
            json!({"type": "tool_result", "result": {"callId": "install-action", "tool": "mcp", "ok": true, "result": {"private": "secret"}}}),
            json!({"type": "approval_required", "action": {"type": "mcp_tool_call", "approval": {"private": "secret"}}}),
        ] {
            assert!(!project_checkpoint_activity(&mut projection, &event, 2));
            assert_eq!(projection, before);
        }
    }

    struct DiskFixture {
        connection: Option<rusqlite::Connection>,
        directory: std::path::PathBuf,
        conversation: crate::storage::models::ChatConversationRecord,
    }

    impl Drop for DiskFixture {
        fn drop(&mut self) {
            drop(self.connection.take());
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn disk_fixture(status: &str, result: Option<Value>) -> DiskFixture {
        use crate::storage::{
            agent_action_audit_repository, chat_repository, migrations, pending_action_repository,
        };
        let directory =
            std::env::temp_dir().join(format!("skill-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut connection = rusqlite::Connection::open(directory.join("fixture.sqlite3")).unwrap();
        migrations::run_migrations(&connection).unwrap();
        let contract: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/protocol/fixtures/persisted-agent-run-projection-v1.json"
        )))
        .unwrap();
        let mut run = contract["expectedCanonical"].clone();
        run["runId"] = json!("run");
        run["status"] = json!("running");
        run.as_object_mut().unwrap().remove("completedAt");
        run.as_object_mut().unwrap().remove("state");
        let conversation: crate::storage::models::ChatConversationRecord =
            serde_json::from_value(json!({
                "id": "conversation", "projectId": null, "modelId": null, "title": "Fixture",
                "createdAt": 1, "updatedAt": 1, "pinnedAt": null, "archivedAt": null, "unreadAt": null,
                "messages": [{
                    "id": "assistant", "role": "assistant", "content": "", "createdAt": 1,
                    "status": "pending", "attachments": [], "folderReferencesJson": null,
                    "agentRunJson": serde_json::to_string(&run).unwrap(), "uiStateJson": null,
                }],
            }))
            .unwrap();
        chat_repository::save_conversation(&mut connection, conversation.clone()).unwrap();
        connection.execute(
            "INSERT INTO conversation_turn_traces (assistant_message_id, conversation_id, run_id,
             schema_version, terminal_status, terminal_error, truncated, created_at, updated_at, completed_at)
             VALUES ('assistant', 'conversation', 'run', 1, 'in_progress', NULL, 0, 1, 1, NULL)",
            [],
        ).unwrap();
        let action_id = crate::canonical_pending_action_id("run", "install-action");
        let action_json = serde_json::to_string(&approval()).unwrap();
        pending_action_repository::store_pending_action(
            &connection,
            &crate::storage::models::AgentPendingActionRecord {
                action_id: action_id.clone(),
                run_id: "run".into(),
                conversation_id: Some("conversation".into()),
                assistant_message_id: Some("assistant".into()),
                action_type: "skill_installation".into(),
                tool_name: "skills_commit_install".into(),
                tool_call_id: Some("install-action".into()),
                status: status.into(),
                target_status: None,
                action_json: action_json.clone(),
                agent_input_json: "{}".into(),
                created_at: 1,
                updated_at: 2,
            },
        )
        .unwrap();
        agent_action_audit_repository::upsert_action_audit_record(
            &connection,
            &crate::storage::models::AgentActionAuditRecord {
                action_id,
                run_id: "run".into(),
                conversation_id: Some("conversation".into()),
                assistant_message_id: Some("assistant".into()),
                action_type: "skill_installation".into(),
                tool_name: "skills_commit_install".into(),
                decision: Some("approved".into()),
                status: status.into(),
                action_json,
                file_change_result_json: None,
                command_result_json: None,
                tool_result_json: result.map(|result| serde_json::to_string(&result).unwrap()),
                error: None,
                created_at: 1,
                decided_at: Some(2),
                completed_at: Some(3),
                effective_permissions_json: None,
                path_scope: None,
                command_cwd_scope: None,
                blocked_reason: None,
                decision_source: Some("manual".into()),
            },
        )
        .unwrap();
        DiskFixture {
            connection: Some(connection),
            directory,
            conversation,
        }
    }

    fn installation_status(conversation: &crate::storage::models::ChatConversationRecord) -> Value {
        let run: Value =
            serde_json::from_str(conversation.messages[0].agent_run_json.as_deref().unwrap())
                .unwrap();
        run["skillInstallations"][0]["status"].clone()
    }

    #[test]
    fn disk_history_restores_out_of_emitter_approval_receipt_without_writing_or_executing() {
        let mut fixture = disk_fixture(
            "completed",
            Some(json!({
                "callId": "install-action", "tool": "skills_commit_install", "ok": true,
                "result": {"status": "alreadyInstalled", "private": "must-not-project"},
            })),
        );
        let connection = fixture.connection.as_ref().unwrap();
        let before = connection.total_changes();
        attach_skill_installation_history(connection, &mut fixture.conversation).unwrap();
        assert_eq!(
            installation_status(&fixture.conversation),
            "already_installed"
        );
        assert!(!fixture.conversation.messages[0]
            .agent_run_json
            .as_deref()
            .unwrap()
            .contains("must-not-project"));
        attach_skill_installation_history(connection, &mut fixture.conversation).unwrap();
        assert_eq!(connection.total_changes(), before);
        assert_eq!(
            installation_status(&fixture.conversation),
            "already_installed"
        );
    }

    #[test]
    fn disk_history_requires_same_trace_run_and_audit_binding() {
        let mut fixture = disk_fixture(
            "completed",
            Some(json!({
                "callId": "install-action", "tool": "skills_commit_install", "ok": true,
                "result": {"status": "installed"},
            })),
        );
        let connection = fixture.connection.as_ref().unwrap();
        connection
            .execute("UPDATE agent_action_audit SET run_id='other-run'", [])
            .unwrap();
        attach_skill_installation_history(connection, &mut fixture.conversation).unwrap();
        assert_eq!(installation_status(&fixture.conversation), "uncertain");

        connection
            .execute("UPDATE agent_pending_actions SET run_id='other-run'", [])
            .unwrap();
        let mut run: Value = serde_json::from_str(
            fixture.conversation.messages[0]
                .agent_run_json
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        run.as_object_mut().unwrap().remove("skillInstallations");
        fixture.conversation.messages[0].agent_run_json =
            Some(serde_json::to_string(&run).unwrap());
        attach_skill_installation_history(connection, &mut fixture.conversation).unwrap();
        let run: Value = serde_json::from_str(
            fixture.conversation.messages[0]
                .agent_run_json
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        assert!(run.get("skillInstallations").is_none());
    }

    #[test]
    fn disk_history_recovers_pending_cancelled_and_uncertain_installations() {
        for (status, result, expected) in [
            ("pending", None, "waiting_for_approval"),
            ("executing", None, "installing"),
            ("cancelled", None, "rejected"),
            (
                "failed",
                Some(json!({
                    "callId": "install-action", "tool": "skills_commit_install", "ok": false,
                    "result": {"commitMayHaveSucceeded": true},
                })),
                "uncertain",
            ),
        ] {
            let mut fixture = disk_fixture(status, result);
            attach_skill_installation_history(
                fixture.connection.as_ref().unwrap(),
                &mut fixture.conversation,
            )
            .unwrap();
            assert_eq!(installation_status(&fixture.conversation), expected);
        }
    }
}
