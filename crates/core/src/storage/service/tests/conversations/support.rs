//! Shared conversation fixtures and one thread-local SQL trace collector.

use super::*;

std::thread_local! {
    static TRACED_STORAGE_SQL: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn record_storage_sql(event: rusqlite::trace::TraceEvent<'_>) {
    let rusqlite::trace::TraceEvent::Stmt(statement, _) = event else {
        return;
    };
    let sql = statement.expanded_sql().expect("test SQL should expand");
    TRACED_STORAGE_SQL.with(|statements| statements.borrow_mut().push(sql.to_string()));
}

pub(in crate::storage::service::tests) fn trace_storage_selects<T>(
    service: &StorageService,
    operation: impl FnOnce() -> T,
) -> (T, Vec<String>) {
    TRACED_STORAGE_SQL.with(|statements| statements.borrow_mut().clear());
    {
        let connection = service.state.connection().unwrap();
        connection.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
            Some(record_storage_sql),
        );
    }
    service
        .state
        .configure_read_connections_for_test(|connection| {
            connection.trace_v2(
                rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
                Some(record_storage_sql),
            )
        });
    let output = operation();
    service
        .state
        .configure_read_connections_for_test(|connection| {
            connection.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None)
        });
    {
        let connection = service.state.connection().unwrap();
        connection.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
    }
    let selects = TRACED_STORAGE_SQL.with(|statements| {
        std::mem::take(&mut *statements.borrow_mut())
            .into_iter()
            .filter(|sql| sql.trim_start().to_ascii_uppercase().starts_with("SELECT"))
            .collect()
    });
    (output, selects)
}

pub(super) fn browser_tool_identity(
    tool_id: &str,
    raw_name: &str,
    model_name: &str,
) -> crate::AgentToolIdentity {
    crate::AgentToolIdentity::BuiltinCapability {
        capability_id: "browser_automation".into(),
        managed_mcp_id: "builtin.browser_automation.mcp".into(),
        package_name: "@playwright/mcp".into(),
        package_version: "0.0.79".into(),
        upstream_catalog_digest: format!("sha256:{}", "1".repeat(64)).into_boxed_str(),
        policy_digest: format!("sha256:{}", "2".repeat(64)).into_boxed_str(),
        manifest_digest: format!("sha256:{}", "3".repeat(64)).into_boxed_str(),
        tool_id: tool_id.into(),
        raw_name: raw_name.to_string().into_boxed_str(),
        model_name: model_name.to_string().into_boxed_str(),
        upstream_schema_digest: format!("sha256:{}", "4".repeat(64)).into_boxed_str(),
        host_overlay_digest: format!("sha256:{}", "5".repeat(64)).into_boxed_str(),
        host_input_schema_digest: format!("sha256:{}", "6".repeat(64)).into_boxed_str(),
    }
}

pub(super) fn current_projection_with_suffix(
    run_id: Option<&str>,
    suffix: &str,
) -> serde_json::Value {
    serde_json::json!({
        "runId": run_id,
        "status": "running",
        "startedAt": 2,
        "toolDefinitions": [],
        "toolCalls": [],
        "toolResults": [],
        "webSearchActivities": [],
        "readActivities": [],
        "approvals": [],
        "fileChangeProposals": [],
        "fileChanges": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {
            "stream-current": {
                "previousContent": "partial response"
            }
        },
        "timeline": [{
            "id": "renderer-suffix",
            "type": "message",
            "content": suffix
        }],
        "interruption": {
            "reason": "service_connection_failed"
        }
    })
}

pub(super) fn current_artifact_observation() -> crate::AgentCommandArtifactObservation {
    serde_json::from_value(serde_json::json!({
        "schemaVersion": crate::AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
        "status": "complete",
        "partial": false,
        "stopReasons": [],
        "scanned": 1,
        "returned": 1,
        "omitted": 0,
        "coverage": {
            "workspaceIncluded": true,
            "expectedOutputCount": 1,
            "additionalRootCount": 0,
            "before": {
                "rootsScanned": 1,
                "directoryEntriesScanned": 1,
                "officeFilesSeen": 0,
                "filesHashed": 0,
                "filesUnhashed": 0,
                "bytesHashed": 0,
                "symlinksSkipped": 0,
                "excludedDirectories": 0,
                "durationMs": 1,
                "timeBudgetExceeded": false,
                "cancelled": false,
                "truncated": false
            },
            "after": {
                "rootsScanned": 1,
                "directoryEntriesScanned": 2,
                "officeFilesSeen": 1,
                "filesHashed": 1,
                "filesUnhashed": 0,
                "bytesHashed": 1024,
                "symlinksSkipped": 0,
                "excludedDirectories": 0,
                "durationMs": 2,
                "timeBudgetExceeded": false,
                "cancelled": false,
                "truncated": false
            }
        },
        "changes": [{
            "kind": "created",
            "artifactKind": "presentation",
            "path": "reports/child-report.pptx",
            "scope": "workspace",
            "after": {
                "sizeBytes": 1024,
                "sha256": "a".repeat(64),
                "validation": { "status": "valid" }
            }
        }],
        "changesTruncated": false,
        "changesOmitted": 0,
        "expectedOutputs": [{
            "requestedPath": "reports/child-report.pptx",
            "outcome": "created",
            "path": "reports/child-report.pptx",
            "scope": "workspace",
            "artifactKind": "presentation",
            "metadata": {
                "sizeBytes": 1024,
                "sha256": "a".repeat(64),
                "validation": { "status": "valid" }
            }
        }],
        "warnings": []
    }))
    .unwrap()
}

pub(super) fn empty_projection_trace(
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: Vec::new(),
    }
}

pub(super) fn save_projection_conversation(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run: serde_json::Value,
) {
    let mut stored = conversation(conversation_id, None, "message-user");
    stored.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: "pending".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: Some(run.to_string()),
        ui_state_json: None,
    });
    service.save_conversation(stored).unwrap();
}

pub(super) fn load_projected_run(
    service: &StorageService,
    conversation_id: &str,
) -> serde_json::Value {
    let view = service
        .load_conversation_view(conversation_id)
        .unwrap()
        .unwrap();
    serde_json::from_str(
        view.conversation
            .messages
            .last()
            .unwrap()
            .agent_run_json
            .as_deref()
            .unwrap(),
    )
    .unwrap()
}

pub(super) fn seed_batched_conversation_projection(
    service: &StorageService,
    conversation_id: &str,
    assistant_count: usize,
) {
    let mut stored = conversation(conversation_id, None, &format!("{conversation_id}-user"));
    for index in 0..assistant_count {
        stored.messages.push(ChatMessageRecord {
            human_interaction_response: None,
            id: format!("{conversation_id}-assistant-{index}"),
            role: "assistant".to_string(),
            content: format!("answer {index}"),
            created_at: i64::try_from(index + 2).unwrap(),
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        });
    }
    service.save_conversation(stored).unwrap();

    for index in 0..assistant_count {
        let assistant_message_id = format!("{conversation_id}-assistant-{index}");
        let run_id = format!("{conversation_id}-run-{index}");
        let mut trace = empty_projection_trace(conversation_id, &assistant_message_id, &run_id);
        trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Completed;
        service
            .replace_conversation_turn_trace(
                &trace,
                i64::try_from(index + 2).unwrap(),
                i64::try_from(index + 2).unwrap(),
            )
            .unwrap();
    }

    let connection = service.state.connection().unwrap();
    for index in 0..assistant_count {
        let assistant_message_id = format!("{conversation_id}-assistant-{index}");
        let run_id = format!("{conversation_id}-run-{index}");
        let attachment_id = format!("{conversation_id}-attachment-{index}");
        let guidance_id = format!("{conversation_id}-guidance-{index}");
        let created_at = i64::try_from(index + 2).unwrap();
        connection
            .execute(
                "INSERT INTO attachments (
                     id, conversation_id, message_id, project_id, kind, original_name,
                     mime_type, size_bytes, storage_rel_path, created_at
                 ) VALUES (?1, ?2, ?3, NULL, 'file', ?4, 'text/plain', 1, ?5, ?6)",
                rusqlite::params![
                    &attachment_id,
                    conversation_id,
                    &assistant_message_id,
                    format!("note-{index}.txt"),
                    format!("{conversation_id}/{attachment_id}/note.txt"),
                    created_at,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO agent_run_guidances (
                     guidance_id, client_message_id, run_id, conversation_id,
                     assistant_message_id, content, status, applied_trace_sequence,
                     terminal_reason, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', NULL, NULL, ?7, ?7)",
                rusqlite::params![
                    &guidance_id,
                    format!("{conversation_id}-client-{index}"),
                    &run_id,
                    conversation_id,
                    &assistant_message_id,
                    format!("guidance {index}"),
                    created_at,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO agent_run_guidance_attachments (
                     guidance_id, attachment_id, position
                 ) VALUES (?1, ?2, 0)",
                rusqlite::params![&guidance_id, &attachment_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO agent_pending_actions (
                     action_id, run_id, conversation_id, assistant_message_id, action_type,
                     tool_name, tool_call_id, status, target_status, action_json,
                     agent_input_json, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, 'mcp_tool_call', 'mcp', ?5, 'pending', NULL,
                           '{}', '{}', ?6, ?6)",
                rusqlite::params![
                    format!("{conversation_id}-action-{index}"),
                    &run_id,
                    conversation_id,
                    &assistant_message_id,
                    format!("{conversation_id}-call-{index}"),
                    created_at,
                ],
            )
            .unwrap();
    }
}

pub(super) fn save_forkable_command_conversation(service: &StorageService, conversation_id: &str) {
    service
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "fork command source".to_string(),
            messages: vec![
                ("user", "1", 1),
                ("assistant", "1", 2),
                ("user", "2", 3),
                ("assistant", "2", 4),
            ]
            .into_iter()
            .map(|(role, suffix, created_at)| ChatMessageRecord {
                human_interaction_response: None,
                id: format!(
                    "{role}-{conversation_id_suffix}-{suffix}",
                    conversation_id_suffix = conversation_id.trim_start_matches("conversation-")
                ),
                role: role.to_string(),
                content: format!("{role} {suffix}"),
                created_at,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: (role == "assistant").then(|| {
                    serde_json::json!({
                        "runId": format!("run-{conversation_id}-{suffix}"),
                        "status": "completed"
                    })
                    .to_string()
                }),
                ui_state_json: None,
            })
            .collect(),
            created_at: 1,
            updated_at: 4,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
}

pub(super) fn fork_test_command_session_create(
    session_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    started_at: u64,
) -> crate::storage::agent_command_session_repository::AgentCommandSessionCreate {
    crate::storage::agent_command_session_repository::AgentCommandSessionCreate {
        snapshot: crate::AgentCommandSessionSnapshot {
            schema_version: crate::storage::agent_command_session_repository::AGENT_COMMAND_SESSION_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            origin_run_id: format!("run-{session_id}"),
            call_id: format!("call-{session_id}"),
            project_id: None,
            command: "python3 app.py".to_string(),
            cwd: "/tmp".to_string(),
            command_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            status: crate::AgentCommandSessionStatus::Starting,
            started_at,
            ended_at: None,
            exit_code: None,
            latest_sequence: 0,
            output_truncated: false,
            outputs: Vec::new(),
            artifact_observation: None,
            archive_ref: None,
        },
        authorization_source: crate::command::CommandAuthorizationSource::ExplicitUser,
        approval_provenance: serde_json::json!({ "decision": "approved" }),
        permission_provenance: serde_json::json!({ "mode": "default" }),
        created_at: i64::try_from(started_at).unwrap(),
    }
}

pub(super) fn create_fork_test_command_session(
    service: &StorageService,
    session_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    started_at: u64,
) {
    service
        .create_agent_command_session(&fork_test_command_session_create(
            session_id,
            conversation_id,
            assistant_message_id,
            started_at,
        ))
        .unwrap();
}

pub(super) fn settle_fork_test_command_session(
    service: &StorageService,
    session_id: &str,
    conversation_id: &str,
    status: crate::AgentCommandSessionStatus,
    timestamp: u64,
) {
    service
        .settle_agent_command_session(
            &crate::storage::agent_command_session_repository::AgentCommandSessionTerminalUpdate {
                conversation_id,
                session_id,
                status,
                ended_at: timestamp,
                exit_code: (status == crate::AgentCommandSessionStatus::Exited).then_some(0),
                latest_sequence: 0,
                transcript_truncated: false,
                output_capture_truncated: false,
                archive_ref: None,
                terminal_reason: None,
                published_outputs: &[],
                artifact_observation: None,
                committed_at: i64::try_from(timestamp).unwrap(),
            },
        )
        .unwrap();
}

pub(super) fn history_timeline_projection_count(
    connection: &rusqlite::Connection,
    conversation_id: &str,
) -> usize {
    let projected = connection.prepare(
        "SELECT entry_rowid, position, within_message_order FROM conversation_history_timeline WHERE conversation_id=?1 ORDER BY entry_rowid"
    ).unwrap().query_map([conversation_id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)))
        .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    let indexed = connection.prepare(
        "SELECT rowid, position, within_message_order FROM conversation_history_fts WHERE conversation_id=?1 AND record_type IN ('message','trace_item') ORDER BY rowid"
    ).unwrap().query_map([conversation_id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)))
        .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(projected, indexed);
    projected.len()
}
