use super::*;
use std::cell::RefCell;

std::thread_local! {
    static TRACED_STORAGE_SQL: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn record_storage_sql(sql: &str) {
    TRACED_STORAGE_SQL.with(|statements| statements.borrow_mut().push(sql.to_string()));
}

fn trace_storage_selects<T>(
    service: &StorageService,
    operation: impl FnOnce() -> T,
) -> (T, Vec<String>) {
    TRACED_STORAGE_SQL.with(|statements| statements.borrow_mut().clear());
    {
        let mut connection = service.state.connection().unwrap();
        connection.trace(Some(record_storage_sql));
    }
    let output = operation();
    {
        let mut connection = service.state.connection().unwrap();
        connection.trace(None);
    }
    let selects = TRACED_STORAGE_SQL.with(|statements| {
        std::mem::take(&mut *statements.borrow_mut())
            .into_iter()
            .filter(|sql| sql.trim_start().to_ascii_uppercase().starts_with("SELECT"))
            .collect()
    });
    (output, selects)
}

fn browser_tool_identity(
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

fn current_projection_with_suffix(run_id: Option<&str>, suffix: &str) -> serde_json::Value {
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

fn empty_projection_trace(
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

fn save_projection_conversation(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run: serde_json::Value,
) {
    let mut stored = conversation(conversation_id, None, "message-user");
    stored.messages.push(ChatMessageRecord {
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: "pending".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: Some(run.to_string()),
        ui_state_json: None,
    });
    service.save_conversation(stored).unwrap();
}

fn load_projected_run(service: &StorageService, conversation_id: &str) -> serde_json::Value {
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

fn seed_batched_conversation_projection(
    service: &StorageService,
    conversation_id: &str,
    assistant_count: usize,
) {
    let mut stored = conversation(conversation_id, None, &format!("{conversation_id}-user"));
    for index in 0..assistant_count {
        stored.messages.push(ChatMessageRecord {
            id: format!("{conversation_id}-assistant-{index}"),
            role: "assistant".to_string(),
            content: format!("answer {index}"),
            created_at: i64::try_from(index + 2).unwrap(),
            status: Some("pending".to_string()),
            attachments: Vec::new(),
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

#[test]
fn conversation_meta_projection_filters_child_agents_in_one_statement() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for id in ["meta-ordinary", "meta-root"] {
        service
            .save_conversation(conversation(id, None, &format!("{id}-message")))
            .unwrap();
    }
    let mut child = conversation("meta-child", None, "meta-child-message");
    child.messages.clear();
    service.save_conversation(child).unwrap();
    bind_agent_root(&service, "meta-root-agent", "meta-root");
    bind_agent_child(
        &service,
        "meta-child-agent",
        "meta-child",
        "meta-root-agent",
        "meta-root",
        "child",
    );

    let (metas, selects) =
        trace_storage_selects(&service, || service.load_conversation_metas().unwrap());
    let mut ids = metas.into_iter().map(|meta| meta.id).collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["meta-ordinary".to_string(), "meta-root".to_string()]);
    assert_eq!(selects.len(), 1, "unexpected metadata SQL: {selects:#?}");
    assert!(selects[0].contains("NOT EXISTS"));
    assert!(selects[0].contains("agent_nodes"));
}

#[test]
fn conversation_detail_projection_query_count_is_independent_of_history_length() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed_batched_conversation_projection(&service, "projection-one", 1);
    seed_batched_conversation_projection(&service, "projection-many", 12);

    let (one, one_selects) = trace_storage_selects(&service, || {
        service
            .load_conversation("projection-one")
            .unwrap()
            .unwrap()
    });
    let (many, many_selects) = trace_storage_selects(&service, || {
        service
            .load_conversation("projection-many")
            .unwrap()
            .unwrap()
    });

    assert_eq!(
        one_selects.len(),
        many_selects.len(),
        "one: {one_selects:#?}\nmany: {many_selects:#?}"
    );
    assert_eq!(
        many_selects.len(),
        13,
        "unexpected detail SQL: {many_selects:#?}"
    );
    for table in [
        "conversation_turn_trace_items",
        "agent_run_guidances",
        "agent_run_guidance_attachments",
        "agent_pending_actions",
        "attachments",
    ] {
        assert!(
            many_selects.iter().any(|sql| sql.contains(table)),
            "missing batched {table} query: {many_selects:#?}"
        );
    }

    assert_eq!(one.messages.len(), 2);
    assert_eq!(many.messages.len(), 13);
    for message in many
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
    {
        assert!(message.attachments.is_empty());
        let run: serde_json::Value =
            serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
        assert_eq!(run["timeline"].as_array().unwrap().len(), 1);
        assert_eq!(run["timeline"][0]["status"], "queued");
        assert_eq!(
            run["timeline"][0]["attachments"].as_array().unwrap().len(),
            1
        );
    }
}

#[test]
fn conversation_view_preserves_a_strict_current_projection_suffix() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-current-projection";
    let assistant_message_id = "assistant-current-projection";
    let run_id = "run-current-projection";
    save_projection_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        current_projection_with_suffix(Some(run_id), "preserve current suffix"),
    );
    service
        .append_in_progress_conversation_turn_trace(
            &empty_projection_trace(conversation_id, assistant_message_id, run_id),
            2,
            2,
        )
        .unwrap();

    let run = load_projected_run(&service, conversation_id);
    assert_eq!(run["runId"], run_id);
    assert_eq!(run["timeline"][0]["content"], "preserve current suffix");
    assert_eq!(
        run["messageStreamCheckpoints"]["stream-current"]["previousContent"],
        "partial response"
    );
    assert_eq!(run["interruption"]["reason"], "service_connection_failed");
    assert_eq!(run["fileChangeProposals"], serde_json::json!([]));
    assert_eq!(run["fileChanges"], serde_json::json!([]));
}

#[test]
fn conversation_view_rebuilds_a_projection_owned_by_another_run() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-mismatched-projection";
    let assistant_message_id = "assistant-mismatched-projection";
    let authoritative_run_id = "run-authoritative-projection";
    save_projection_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        current_projection_with_suffix(Some("run-wrong-owner"), "must not survive"),
    );
    service
        .append_in_progress_conversation_turn_trace(
            &empty_projection_trace(conversation_id, assistant_message_id, authoritative_run_id),
            2,
            2,
        )
        .unwrap();

    let run = load_projected_run(&service, conversation_id);
    assert_eq!(run["runId"], authoritative_run_id);
    assert_eq!(run["timeline"], serde_json::json!([]));
    assert!(run.get("interruption").is_none());
    assert_eq!(run["messageStreamCheckpoints"], serde_json::json!({}));
}

#[test]
fn conversation_view_binds_a_current_null_run_id_to_the_durable_trace() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-null-run-projection";
    let assistant_message_id = "assistant-null-run-projection";
    let run_id = "run-bound-projection";
    save_projection_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        current_projection_with_suffix(None, "preserve null-owner suffix"),
    );
    service
        .append_in_progress_conversation_turn_trace(
            &empty_projection_trace(conversation_id, assistant_message_id, run_id),
            2,
            2,
        )
        .unwrap();

    let run = load_projected_run(&service, conversation_id);
    assert_eq!(run["runId"], run_id);
    assert_eq!(run["timeline"][0]["content"], "preserve null-owner suffix");
}

#[test]
fn conversation_view_rejects_guidance_rows_with_conflicting_run_owners() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-conflicting-guidance";
    let assistant_message_id = "assistant-conflicting-guidance";
    save_projection_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        current_projection_with_suffix(Some("run-guidance-a"), "must not load"),
    );
    for (index, run_id) in ["run-guidance-a", "run-guidance-b"].into_iter().enumerate() {
        service
            .store_agent_run_guidance(AgentRunGuidanceRecord {
                guidance_id: format!("guidance-conflict-{index}"),
                client_message_id: format!("client-guidance-conflict-{index}"),
                run_id: run_id.to_string(),
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                content: format!("guidance {index}"),
                status: crate::AgentGuidanceStatus::Queued,
                attachment_ids: Vec::new(),
                applied_trace_sequence: None,
                terminal_reason: None,
                created_at: 3 + index as i64,
                updated_at: 3 + index as i64,
            })
            .unwrap();
    }

    assert_eq!(
        service.load_conversation_view(conversation_id).unwrap_err(),
        "guidance projection has inconsistent run identity"
    );
}

#[test]
fn loading_a_backend_owned_turn_projects_durable_tool_activity_without_renderer_writes() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut stored = conversation("conversation-observer-trace", None, "message-user");
    stored.messages.push(ChatMessageRecord {
        id: "message-assistant".to_string(),
        role: "assistant".to_string(),
        content: "Done".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    service.save_conversation(stored).unwrap();
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-observer".to_string(),
        conversation_id: "conversation-observer-trace".to_string(),
        assistant_message_id: "message-assistant".to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "call-read".to_string(),
                tool: "read_file".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "read_file".to_string(),
                },
                operation: serde_json::json!({ "path": "README.md" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: "call-read".to_string(),
                tool: "read_file".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "content": "bounded" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 2,
                call_id: "call-skill".to_string(),
                tool: "skills_activate".to_string(),
                provenance: crate::AgentToolIdentity::RuntimeExtension {
                    extension_id: "skills".to_string(),
                    tool_name: "skills_activate".to_string(),
                },
                operation: serde_json::json!({ "skillRef": "s_000000000000000000000000" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 3,
                call_id: "call-skill".to_string(),
                tool: "skills_activate".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({
                    "status": "activated",
                    "activationRevision": "activation-sha256-v1:observer",
                    "skill": {
                        "id": "bundled:application:spreadsheets",
                        "name": "Spreadsheets",
                        "revision": "revision-1",
                        "source": "bundled:application:spreadsheets"
                    }
                }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 4,
                call_id: "call-browser-evaluate".to_string(),
                tool: "browser_evaluate".to_string(),
                provenance: browser_tool_identity(
                    "browser.evaluate",
                    "browser_evaluate",
                    "browser_evaluate",
                ),
                operation: serde_json::json!({ "script": "() => document.title" }),
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 5,
                call_id: "call-browser-evaluate".to_string(),
                tool: "browser_evaluate".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "value": "Browser smoke" }),
                approval_status: crate::AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 6,
                call_id: "call-browser-tabs".to_string(),
                tool: "browser_tabs".to_string(),
                provenance: browser_tool_identity("browser.tabs", "browser_tabs", "browser_tabs"),
                operation: serde_json::json!({ "action": "list" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 7,
                call_id: "call-browser-tabs".to_string(),
                tool: "browser_tabs".to_string(),
                status: crate::ConversationTraceToolResultStatus::Failed,
                success: false,
                observation: serde_json::json!({}),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: Some("transport closed".to_string()),
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    service
        .append_in_progress_conversation_turn_trace(&trace, 2, 2)
        .unwrap();

    let loaded = service
        .load_conversation("conversation-observer-trace")
        .unwrap()
        .unwrap();
    let run: serde_json::Value = serde_json::from_str(
        loaded.messages[1]
            .agent_run_json
            .as_deref()
            .expect("durable trace should produce a renderer-safe run"),
    )
    .unwrap();
    assert_eq!(run["runId"], "run-observer");
    assert_eq!(run["toolCalls"][0]["id"], "call-read");
    assert_eq!(run["toolCalls"][0]["args"]["path"], "README.md");
    assert_eq!(run["toolResults"][0]["callId"], "call-read");
    assert_eq!(run["toolResults"][0]["result"]["content"], "bounded");
    assert_eq!(run["timeline"][0]["type"], "tool_call");
    assert_eq!(run["fileChangeProposals"], serde_json::json!([]));
    assert_eq!(run["fileChanges"], serde_json::json!([]));
    assert!(run.get("diffs").is_none());
    assert!(run.get("fileDrafts").is_none());
    assert_eq!(run["activatedSkills"][0]["name"], "Spreadsheets");
    assert_eq!(run["activatedSkills"][0]["source"]["kind"], "bundled");
    assert_eq!(
        run["skillActivationRevision"],
        "activation-sha256-v1:observer"
    );
    for (call_id, tool_id, tool_name, sequence) in [
        (
            "call-browser-evaluate",
            "browser.evaluate",
            "browser_evaluate",
            4,
        ),
        ("call-browser-tabs", "browser.tabs", "browser_tabs", 6),
    ] {
        let timeline_item = run["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["callId"] == call_id)
            .unwrap();
        assert_eq!(timeline_item["identity"]["type"], "builtin_capability");
        assert_eq!(timeline_item["identity"]["modelName"], tool_name);
        assert_eq!(timeline_item["identity"]["rawName"], tool_name);
        assert_eq!(timeline_item["identity"]["toolId"], tool_id);
        assert_eq!(timeline_item["traceSequence"], sequence);
    }
}

#[test]
fn collaboration_root_fork_reopens_with_raw_snapshot_and_accepts_a_new_turn() {
    let fixture = tempfile::tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let service = StorageService::open(&database_path).unwrap();
    let source_conversation_id = "conversation-fork-continue-source";
    let source_assistant_message_id = "assistant-fork-continue-source";
    let mut source = conversation(source_conversation_id, None, "user-fork-continue-source");
    source.messages.push(ChatMessageRecord {
        id: source_assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: "Durable source answer".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        // The renderer Timeline and authoritative Usage are reconstructed around this raw
        // lifecycle projection. Neither derived value belongs in the persisted Fork snapshot.
        agent_run_json: Some(
            crate::storage::chat_repository::canonical_agent_run_lifecycle_projection(
                None,
                "run-fork-continue-source",
                "completed",
                1,
                2,
                Some(2),
            )
            .unwrap(),
        ),
        ui_state_json: None,
    });
    source.updated_at = 2;
    service.save_conversation(source).unwrap();
    service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "agent-fork-continue-source".to_string(),
            conversation_id: source_conversation_id.to_string(),
            creation_request_id: "ensure-fork-continue-source".to_string(),
            task_name: "Fork continuation source".to_string(),
        })
        .unwrap();

    let source_trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-fork-continue-source".to_string(),
        conversation_id: source_conversation_id.to_string(),
        assistant_message_id: source_assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: "Inspecting durable history".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 1,
                call_id: "call-fork-continue".to_string(),
                tool: "read_file".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "read_file".to_string(),
                },
                operation: serde_json::json!({ "path": "README.md" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: "call-fork-continue".to_string(),
                tool: "read_file".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "content": "bounded" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    let mut in_progress = source_trace.clone();
    in_progress.terminal_status = crate::ConversationTurnTraceTerminalStatus::InProgress;
    service
        .append_in_progress_conversation_turn_trace(&in_progress, 2, 2)
        .unwrap();
    service
        .replace_conversation_turn_trace(&source_trace, 2, 3)
        .unwrap();
    let mut source_usage = agent_usage_record(source_conversation_id, source_assistant_message_id);
    source_usage.id = "usage-fork-continue-source".to_string();
    source_usage.run_id = "run-fork-continue-source".to_string();
    service.upsert_agent_usage(source_usage).unwrap();
    let source_view = service
        .load_conversation(source_conversation_id)
        .unwrap()
        .unwrap();
    let source_view_run: serde_json::Value =
        serde_json::from_str(source_view.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(source_view_run["usage"]["totalTokens"], 20);
    assert_eq!(source_view_run["timeline"][0]["type"], "message");
    let (source_admission, _) = service
        .load_conversation_for_turn(source_conversation_id)
        .unwrap();
    let source_admission_run: serde_json::Value = serde_json::from_str(
        source_admission.unwrap().messages[1]
            .agent_run_json
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert!(source_admission_run.get("usage").is_none());
    assert!(source_admission_run["timeline"]
        .as_array()
        .unwrap()
        .is_empty());

    let forked = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-continue-request",
            source_conversation_id,
            source_assistant_message_id,
        ))
        .unwrap()
        .conversation;
    let forked_conversation_id = forked.id.clone();
    let forked_assistant_message_id = forked.messages[1].id.clone();
    let projected_run: serde_json::Value = serde_json::from_str(
        forked.messages[1]
            .agent_run_json
            .as_deref()
            .expect("fork view must project the cloned durable Trace"),
    )
    .unwrap();
    assert_eq!(projected_run["timeline"][0]["type"], "message");
    assert_eq!(projected_run["timeline"][1]["type"], "tool_call");
    assert_eq!(projected_run["usage"]["totalTokens"], 0);

    let persisted_fork_run = {
        let connection = service.state.connection().unwrap();
        connection
            .query_row(
                "SELECT agent_run_json FROM messages
                 WHERE conversation_id = ?1 AND id = ?2",
                rusqlite::params![&forked_conversation_id, &forked_assistant_message_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
    };
    assert_ne!(
        forked.messages[1].agent_run_json, persisted_fork_run,
        "the Fork return value is a renderer projection, not write-back input"
    );

    // Reopen to cover the real application lifecycle: Turn admission must receive the raw
    // immutable snapshot, while a renderer read continues to receive the reconstructed Timeline.
    drop(service);
    let reopened = StorageService::open(&database_path).unwrap();
    let (candidate, revision) = reopened
        .load_conversation_for_turn(&forked_conversation_id)
        .unwrap();
    let mut candidate = candidate.unwrap();
    assert_eq!(candidate.messages[1].agent_run_json, persisted_fork_run);
    let next_created_at = candidate.updated_at.saturating_add(1);
    candidate.updated_at = next_created_at.saturating_add(1);
    candidate.messages.extend([
        ChatMessageRecord {
            id: "user-fork-continue-next".to_string(),
            role: "user".to_string(),
            content: "Continue after the fork".to_string(),
            created_at: next_created_at,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "assistant-fork-continue-next".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: next_created_at.saturating_add(1),
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    let next_trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-fork-continue-next",
        &forked_conversation_id,
        "assistant-fork-continue-next",
    );
    reopened
        .save_conversation_and_begin_turn(
            candidate,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &next_trace,
            next_created_at.saturating_add(1),
            next_created_at.saturating_add(1),
        )
        .unwrap();
    let completed_trace = crate::completed_conversation_trace_without_items(
        "run-fork-continue-next",
        &forked_conversation_id,
        "assistant-fork-continue-next",
    );
    reopened
        .finalize_chat_message_with_conversation_trace(
            &forked_conversation_id,
            "assistant-fork-continue-next",
            "Continued answer",
            Some("sent"),
            "completed",
            &completed_trace,
            next_created_at.saturating_add(1),
            next_created_at.saturating_add(2),
        )
        .unwrap();

    let view = reopened
        .load_conversation(&forked_conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(view.messages.len(), 4);
    assert_eq!(view.messages[3].content, "Continued answer");
    let old_projected_run: serde_json::Value =
        serde_json::from_str(view.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(old_projected_run["timeline"][0]["type"], "message");

    let connection = reopened.state.connection().unwrap();
    let (raw_run_json, origin_kind) = connection
        .query_row(
            "SELECT agent_run_json, input_origin_kind FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![&forked_conversation_id, &forked_assistant_message_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(raw_run_json, persisted_fork_run);
    assert_eq!(origin_kind.as_deref(), Some("snapshot"));
}

#[test]
fn loading_a_backend_owned_turn_joins_terminal_command_session_and_artifact_projection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-observer-command";
    let assistant_message_id = "message-command-assistant";
    let run_id = "run-observer-command";
    let call_id = "call-observer-command";
    let session_id = "cmd_000000000000000000000000000000c1";
    let mut stored = conversation(conversation_id, None, "message-command-user");
    stored.messages.push(ChatMessageRecord {
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: "Report ready".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    service.save_conversation(stored).unwrap();
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "run_command".to_string(),
                },
                operation: serde_json::json!({ "command": "render-report" }),
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "status": "completed" }),
                approval_status: crate::AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    let mut in_progress = trace.clone();
    in_progress.terminal_status = crate::ConversationTurnTraceTerminalStatus::InProgress;
    service
        .append_in_progress_conversation_turn_trace(&in_progress, 2, 2)
        .unwrap();
    service
        .replace_conversation_turn_trace(&trace, 2, 3)
        .unwrap();

    let mut session =
        fork_test_command_session_create(session_id, conversation_id, assistant_message_id, 2);
    session.snapshot.origin_run_id = run_id.to_string();
    session.snapshot.call_id = call_id.to_string();
    service.create_agent_command_session(&session).unwrap();
    let output = crate::command::AgentCommandPublishedOutput {
        name: "reports/child-report.pdf".to_string(),
        kind: crate::command::AgentCommandPublishedOutputKind::Document,
        read_path: format!("artifact://sha256/{}", "a".repeat(64)),
        mime_type: "application/pdf".to_string(),
        size_bytes: 4096,
        sha256: "a".repeat(64),
        width: None,
        height: None,
    };
    service
        .settle_agent_command_session(
            &crate::storage::agent_command_session_repository::AgentCommandSessionTerminalUpdate {
                conversation_id,
                session_id,
                status: crate::AgentCommandSessionStatus::Exited,
                ended_at: 3,
                exit_code: Some(0),
                latest_sequence: 1,
                transcript_truncated: false,
                output_capture_truncated: false,
                archive_ref: None,
                terminal_reason: None,
                published_outputs: &[output],
                artifact_observation: None,
                committed_at: 3,
            },
        )
        .unwrap();

    let loaded = service.load_conversation(conversation_id).unwrap().unwrap();
    let run: serde_json::Value =
        serde_json::from_str(loaded.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["commandSessions"][call_id]["status"], "exited");
    assert_eq!(
        run["commandSessions"][call_id]["outputs"][0]["name"],
        "reports/child-report.pdf"
    );
    assert_eq!(run["commandSessions"][call_id]["exitCode"], 0);
}

#[test]
fn rollback_turn_preparation_removes_only_the_exact_empty_provisional_trace_and_messages() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let previous = conversation("conversation-turn-rollback", None, "message-existing");
    service.save_conversation(previous.clone()).unwrap();
    let mut prepared = previous.clone();
    prepared.model_id = Some("model-2".to_string());
    prepared.updated_at = 10;
    prepared.messages.extend([
        ChatMessageRecord {
            id: "message-provisional-user".to_string(),
            role: "user".to_string(),
            content: "provisional".to_string(),
            created_at: 9,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "message-provisional-assistant".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 10,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    service.save_conversation(prepared).unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-provisional",
        "conversation-turn-rollback",
        "message-provisional-assistant",
    );
    service
        .append_in_progress_conversation_turn_trace(&trace, 10, 10)
        .unwrap();

    service
        .rollback_conversation_turn_preparation(
            "conversation-turn-rollback",
            "message-provisional-user",
            "message-provisional-assistant",
            Some("run-provisional"),
            Some(&previous),
            true,
        )
        .unwrap();

    assert_eq!(
        serde_json::to_value(
            service
                .load_conversation("conversation-turn-rollback")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(previous).unwrap()
    );
    assert!(service
        .get_conversation_turn_trace("message-provisional-assistant")
        .unwrap()
        .is_none());
}

#[test]
fn rollback_turn_preparation_refuses_to_delete_a_nonempty_or_foreign_trace() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let previous = conversation("conversation-turn-rollback-foreign", None, "existing");
    service.save_conversation(previous.clone()).unwrap();
    let mut prepared = previous.clone();
    prepared.messages.push(ChatMessageRecord {
        id: "assistant-foreign".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    service.save_conversation(prepared).unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-owner",
        "conversation-turn-rollback-foreign",
        "assistant-foreign",
    );
    service
        .append_in_progress_conversation_turn_trace(&trace, 2, 2)
        .unwrap();

    let error = service
        .rollback_agent_wake_turn_preparation(
            "conversation-turn-rollback-foreign",
            "assistant-foreign",
            Some("run-not-owner"),
            &previous,
            true,
        )
        .unwrap_err();
    assert!(error.contains("owned by another run"), "{error}");
    assert!(service
        .get_conversation_turn_trace("assistant-foreign")
        .unwrap()
        .is_some());
    assert_eq!(
        service
            .load_conversation("conversation-turn-rollback-foreign")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2
    );
}

#[test]
fn rollback_removes_only_provisional_facts_and_preserves_concurrent_metadata() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let previous = conversation(
        "conversation-turn-rollback-meta",
        None,
        "message-existing-meta",
    );
    service.save_conversation(previous.clone()).unwrap();
    let mut prepared = previous;
    prepared.model_id = Some("model-2".to_string());
    prepared.updated_at = 10;
    prepared.messages.extend([
        ChatMessageRecord {
            id: "message-provisional-meta-user".to_string(),
            role: "user".to_string(),
            content: "provisional".to_string(),
            created_at: 9,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "message-provisional-meta-assistant".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 10,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    service.save_conversation(prepared).unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-provisional-meta",
        "conversation-turn-rollback-meta",
        "message-provisional-meta-assistant",
    );
    service
        .append_in_progress_conversation_turn_trace(&trace, 10, 10)
        .unwrap();

    let mut concurrent = service
        .load_conversation_metas()
        .unwrap()
        .into_iter()
        .find(|conversation| conversation.id == "conversation-turn-rollback-meta")
        .unwrap();
    concurrent.title = "Pinned while preparing".to_string();
    concurrent.pinned_at = Some(11);
    concurrent.updated_at = 11;
    service.save_conversation_meta(concurrent).unwrap();

    service
        .rollback_conversation_turn_preparation(
            "conversation-turn-rollback-meta",
            "message-provisional-meta-user",
            "message-provisional-meta-assistant",
            Some("run-provisional-meta"),
            Some(&conversation(
                "conversation-turn-rollback-meta",
                None,
                "message-existing-meta",
            )),
            true,
        )
        .unwrap();

    let current = service
        .load_conversation("conversation-turn-rollback-meta")
        .unwrap()
        .unwrap();
    assert_eq!(current.title, "Pinned while preparing");
    assert_eq!(current.pinned_at, Some(11));
    assert_eq!(current.updated_at, 11);
    assert_eq!(current.model_id.as_deref(), Some("model-2"));
    assert_eq!(current.messages.len(), 1);
    assert_eq!(current.messages[0].id, "message-existing-meta");
    assert!(service
        .get_conversation_turn_trace("message-provisional-meta-assistant")
        .unwrap()
        .is_none());
}

#[test]
fn stale_full_conversation_snapshot_cannot_delete_an_active_turn_before_unique_check() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let base = conversation("conversation-stale-turn-admission", None, "message-base");
    service.save_conversation(base.clone()).unwrap();
    let (_, initial_revision) = service
        .load_conversation_for_turn("conversation-stale-turn-admission")
        .unwrap();

    let mut candidate_a = base.clone();
    candidate_a.messages.extend([
        ChatMessageRecord {
            id: "user-candidate-a".to_string(),
            role: "user".to_string(),
            content: "candidate a".to_string(),
            created_at: 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "assistant-candidate-a".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 3,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    candidate_a.updated_at = 3;
    let trace_a = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-candidate-a",
        &candidate_a.id,
        "assistant-candidate-a",
    );
    service
        .save_conversation_and_begin_turn(
            candidate_a.clone(),
            initial_revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace_a,
            3,
            3,
        )
        .unwrap();
    let (_, active_revision) = service
        .load_conversation_for_turn("conversation-stale-turn-admission")
        .unwrap();

    // Candidate B was built from the same stale pre-A snapshot. A full-save implementation that
    // checks uniqueness only after replacing messages would cascade-delete A's trace and win.
    let mut candidate_b = base;
    candidate_b.messages.extend([
        ChatMessageRecord {
            id: "user-candidate-b".to_string(),
            role: "user".to_string(),
            content: "candidate b".to_string(),
            created_at: 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "assistant-candidate-b".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 3,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    candidate_b.updated_at = 3;
    let trace_b = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-candidate-b",
        &candidate_b.id,
        "assistant-candidate-b",
    );
    let error = service
        .save_conversation_and_begin_turn(
            candidate_b,
            active_revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace_b,
            3,
            3,
        )
        .unwrap_err();
    assert!(error.contains("active durable Turn"), "{error}");

    let mut current = service
        .load_conversation("conversation-stale-turn-admission")
        .unwrap()
        .unwrap();
    let projected_run: serde_json::Value = serde_json::from_str(
        current.messages[2]
            .agent_run_json
            .as_deref()
            .expect("the durable active trace should project a running lifecycle"),
    )
    .unwrap();
    assert_eq!(projected_run["runId"], "run-candidate-a");
    assert_eq!(projected_run["status"], "running");
    assert_eq!(projected_run["state"]["status"], "running");
    assert_eq!(projected_run["state"]["activeRunId"], "run-candidate-a");
    assert_eq!(projected_run["timeline"], serde_json::json!([]));
    current.messages[2].agent_run_json = None;
    assert_eq!(
        serde_json::to_value(current).unwrap(),
        serde_json::to_value(candidate_a).unwrap()
    );
    let traces = service
        .list_conversation_turn_traces("conversation-stale-turn-admission")
        .unwrap();
    assert_eq!(traces, vec![trace_a]);
    assert!(service
        .get_conversation_turn_trace("assistant-candidate-b")
        .unwrap()
        .is_none());
}

#[test]
fn completed_turn_revision_fences_a_cross_host_stale_full_snapshot() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-completed-turn-cas",
            None,
            "message-base",
        ))
        .unwrap();
    let (base, initial_revision) = service
        .load_conversation_for_turn("conversation-completed-turn-cas")
        .unwrap();
    let base = base.unwrap();

    let mut candidate_a = base.clone();
    candidate_a.messages.extend([
        ChatMessageRecord {
            id: "user-completed-a".to_string(),
            role: "user".to_string(),
            content: "candidate a".to_string(),
            created_at: 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "assistant-completed-a".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 3,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    candidate_a.updated_at = 3;
    let trace_a = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-completed-a",
        &candidate_a.id,
        "assistant-completed-a",
    );
    service
        .save_conversation_and_begin_turn(
            candidate_a,
            initial_revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace_a,
            3,
            3,
        )
        .unwrap();
    let terminal_a = crate::completed_conversation_trace_without_items(
        "run-completed-a",
        "conversation-completed-turn-cas",
        "assistant-completed-a",
    );
    service
        .finalize_chat_message_with_conversation_trace_and_usage(
            "conversation-completed-turn-cas",
            "assistant-completed-a",
            "candidate a complete",
            Some("sent"),
            "completed",
            &terminal_a,
            3,
            4,
            None,
        )
        .unwrap();
    assert!(service
        .list_conversation_turn_traces("conversation-completed-turn-cas")
        .unwrap()
        .iter()
        .all(|trace| {
            trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress
        }));
    let completed_a = service
        .load_conversation("conversation-completed-turn-cas")
        .unwrap()
        .unwrap();

    // Host B loaded `base` before A began. A has already released the active trace, so only the
    // durable Conversation revision can stop B from replacing A's completed messages.
    let mut stale_candidate_b = base;
    stale_candidate_b.messages.extend([
        ChatMessageRecord {
            id: "user-stale-b".to_string(),
            role: "user".to_string(),
            content: "candidate b".to_string(),
            created_at: 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "assistant-stale-b".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 3,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    stale_candidate_b.updated_at = 3;
    let trace_b = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-stale-b",
        &stale_candidate_b.id,
        "assistant-stale-b",
    );
    let error = service
        .save_conversation_and_begin_turn(
            stale_candidate_b,
            initial_revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace_b,
            3,
            3,
        )
        .unwrap_err();
    assert!(error.contains("Conversation changed"), "{error}");
    assert_eq!(
        serde_json::to_value(
            service
                .load_conversation("conversation-completed-turn-cas")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(completed_a).unwrap()
    );
    assert!(service
        .get_conversation_turn_trace("assistant-stale-b")
        .unwrap()
        .is_none());
}

#[test]
fn turn_commit_rechecks_graph_identity_and_lifecycle_inside_the_write_transaction() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-turn-graph-fence";
    service
        .save_conversation(conversation(conversation_id, None, "message-graph-fence"))
        .unwrap();
    let root = service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "agent-turn-graph-fence".to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: "ensure-turn-graph-fence".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap()
        .record()
        .clone();
    let (snapshot, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut candidate = snapshot.unwrap();
    candidate.messages.push(ChatMessageRecord {
        id: "assistant-graph-fence".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-graph-fence",
        conversation_id,
        "assistant-graph-fence",
    );

    // Lifecycle changes do not rewrite the Conversation row itself. Admission must nevertheless
    // re-read the bound Agent under the same IMMEDIATE transaction as the Turn write.
    service
        .transition_agent_lifecycle(
            &root.agent_id,
            root.revision,
            crate::AgentLifecycle::Active,
            crate::AgentLifecycle::Disabled,
        )
        .unwrap();
    let error = service
        .save_conversation_and_begin_turn(
            candidate,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace,
            2,
            2,
        )
        .unwrap_err();
    assert!(error.contains("active root Agent"), "{error}");
    assert!(service
        .get_conversation_turn_trace("assistant-graph-fence")
        .unwrap()
        .is_none());
    assert!(service
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap()
        .messages
        .iter()
        .all(|message| message.id != "assistant-graph-fence"));
}

#[test]
fn root_permission_snapshot_is_atomic_with_turn_admission_and_survives_reopen() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-permission-reopen";
    service
        .save_conversation(conversation(
            conversation_id,
            None,
            "message-permission-reopen-user",
        ))
        .unwrap();
    service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "agent-permission-reopen".to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: "ensure-permission-reopen".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    let (candidate, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut candidate = candidate.unwrap();
    candidate.messages.push(ChatMessageRecord {
        id: "assistant-permission-reopen".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    candidate.updated_at = 2;
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-permission-reopen",
        conversation_id,
        "assistant-permission-reopen",
    );
    let full = crate::AgentPermissions {
        read: crate::AgentReadPermission::All,
        write: crate::AgentWritePermission::All,
        command: crate::AgentCommandPermission::AutoApprove,
        command_safety: crate::AgentCommandSafetyPolicy::FullAccess,
        patch: crate::AgentPatchPermission::AutoApprove,
        builtin_execution: crate::AgentBuiltinExecutionPermission::AutoApprove,
    };
    let (_, admitted) = service
        .save_conversation_and_begin_turn(
            candidate,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(full),
            &trace,
            2,
            2,
        )
        .unwrap();
    assert_eq!(admitted, full);
    drop(service);

    let reopened = fixture.service();
    let snapshot = reopened
        .get_agent_effective_permission_snapshot("agent-permission-reopen")
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.permissions, full);
    assert_eq!(snapshot.source_run_id, "run-permission-reopen");
    assert_eq!(
        snapshot.source_assistant_message_id,
        "assistant-permission-reopen"
    );
}

#[test]
fn deleting_conversation_and_project_removes_composer_drafts() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-1",
            Some("project-1"),
            "message-1",
        ))
        .unwrap();
    service
        .save_conversation(conversation(
            "conversation-2",
            Some("project-1"),
            "message-2",
        ))
        .unwrap();
    service
        .save_conversation(conversation(
            "conversation-3",
            Some("project-2"),
            "message-3",
        ))
        .unwrap();

    service
        .save_composer_draft(composer_draft(
            "conversation-1",
            Some("project-1"),
            "draft 1",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "conversation-2",
            Some("project-1"),
            "draft 2",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "new-conversation-project-1",
            Some("project-1"),
            "new draft",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "conversation-3",
            Some("project-2"),
            "draft 3",
        ))
        .unwrap();

    service.delete_conversation("conversation-1").unwrap();

    let mut scopes = service
        .load_composer_drafts()
        .unwrap()
        .into_iter()
        .map(|draft| draft.scope_id)
        .collect::<Vec<_>>();
    scopes.sort();
    assert_eq!(
        scopes,
        vec![
            "conversation-2".to_string(),
            "conversation-3".to_string(),
            "new-conversation-project-1".to_string()
        ]
    );

    service.delete_project("project-1").unwrap();

    let mut scopes = service
        .load_composer_drafts()
        .unwrap()
        .into_iter()
        .map(|draft| draft.scope_id)
        .collect::<Vec<_>>();
    scopes.sort();
    assert_eq!(scopes, vec!["conversation-3".to_string()]);
}

#[test]
fn graph_bound_root_conversation_and_project_deletes_remove_owned_agent_trees() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for (project_id, suffix) in [("project-1", "conversation"), ("project-2", "project")] {
        let conversation_id = format!("conversation-graph-{suffix}");
        let message_id = format!("message-graph-{suffix}");
        let agent_id = format!("agent-graph-{suffix}");
        service
            .save_conversation(conversation(
                &conversation_id,
                Some(project_id),
                &message_id,
            ))
            .unwrap();
        service
            .save_composer_draft(composer_draft(
                &conversation_id,
                Some(project_id),
                "owned draft",
            ))
            .unwrap();
        service
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id,
                conversation_id,
                creation_request_id: format!("ensure-agent-graph-{suffix}"),
                task_name: "Root".to_string(),
            })
            .unwrap();
    }

    service
        .delete_conversation("conversation-graph-conversation")
        .unwrap();
    assert!(service
        .load_conversation("conversation-graph-conversation")
        .unwrap()
        .is_none());
    assert!(service
        .get_agent_node("agent-graph-conversation")
        .unwrap()
        .is_none());
    assert!(service
        .load_projects()
        .unwrap()
        .iter()
        .any(|project| project.id == "project-1"));

    service.delete_project("project-2").unwrap();
    assert!(service
        .load_conversation("conversation-graph-project")
        .unwrap()
        .is_none());
    assert!(service
        .get_agent_node("agent-graph-project")
        .unwrap()
        .is_none());
    assert!(service
        .load_projects()
        .unwrap()
        .iter()
        .all(|project| project.id != "project-2"));
    assert!(service
        .load_composer_drafts()
        .unwrap()
        .iter()
        .all(|draft| !draft.scope_id.starts_with("conversation-graph-")));

    let connection = service.state.connection().unwrap();
    let violations = connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn graph_bound_deletes_preserve_and_block_referencing_automations() {
    use crate::storage::automation_repository::{
        AutomationConfigRecord, AutomationCreateOutcome, NewAutomationRecord,
        StoredAutomationStatus,
    };

    let fixture = StorageFixture::new();
    let service = fixture.service();
    let create_referencing_automation =
        |automation_id: &str, request_id: &str, conversation_id: &str, project_id: &str| {
            match service
                .create_automation(&NewAutomationRecord {
                    id: automation_id.to_string(),
                    create_request_id: request_id.to_string(),
                    status: StoredAutomationStatus::Active,
                    config: AutomationConfigRecord {
                        title: format!("Follow {conversation_id}"),
                        prompt: "Report important changes.".to_string(),
                        health_state: "ok".to_string(),
                        blocked_code: None,
                        blocked_message: None,
                        destination_kind: "existing_chat".to_string(),
                        target_conversation_id: Some(conversation_id.to_string()),
                        project_binding_kind: "inherit".to_string(),
                        project_id: None,
                        model_id: None,
                        permission_mode: "default".to_string(),
                        permission_mode_version: 2,
                        permissions_json: "{}".to_string(),
                        reasoning_json: None,
                        schedule_kind: "daily".to_string(),
                        schedule_json:
                            r#"{"kind":"daily","timeMinutes":540,"timezone":"Asia/Shanghai"}"#
                                .to_string(),
                        rrule: "FREQ=DAILY;INTERVAL=1".to_string(),
                        timezone: "Asia/Shanghai".to_string(),
                        anchor_at: 1_700_000_000_000,
                        next_run_at: Some(1_700_000_100_000),
                        notification_policy: "important_updates".to_string(),
                        target_project_snapshot: Some(project_id.to_string()),
                        target_conversation_snapshot: Some(conversation_id.to_string()),
                        target_model_snapshot: Some("model-1".to_string()),
                        target_project_id_snapshot: Some(project_id.to_string()),
                        target_conversation_id_snapshot: Some(conversation_id.to_string()),
                        target_model_id_snapshot: Some("model-1".to_string()),
                    },
                })
                .unwrap()
            {
                AutomationCreateOutcome::Created(record) => record,
                outcome => panic!("unexpected create outcome: {outcome:?}"),
            }
        };

    for (project_id, suffix) in [
        ("project-1", "referenced-conversation"),
        ("project-2", "referenced-project"),
    ] {
        let conversation_id = format!("conversation-graph-{suffix}");
        service
            .save_conversation(conversation(
                &conversation_id,
                Some(project_id),
                &format!("message-{suffix}"),
            ))
            .unwrap();
        service
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id: format!("agent-graph-{suffix}"),
                conversation_id: conversation_id.clone(),
                creation_request_id: format!("ensure-agent-graph-{suffix}"),
                task_name: "Root".to_string(),
            })
            .unwrap();
        create_referencing_automation(
            &format!("automation-{suffix}"),
            &format!("automation-request-{suffix}"),
            &conversation_id,
            project_id,
        );
    }

    service
        .delete_conversation("conversation-graph-referenced-conversation")
        .unwrap();
    let conversation_blocked = service
        .get_automation("automation-referenced-conversation")
        .unwrap()
        .unwrap();
    assert_eq!(conversation_blocked.status, StoredAutomationStatus::Active);
    assert_eq!(conversation_blocked.config.health_state, "blocked");
    assert_eq!(
        conversation_blocked.config.blocked_code.as_deref(),
        Some("target_missing")
    );
    assert!(conversation_blocked.config.target_conversation_id.is_none());
    assert!(conversation_blocked.config.next_run_at.is_none());

    service.delete_project("project-2").unwrap();
    let project_blocked = service
        .get_automation("automation-referenced-project")
        .unwrap()
        .unwrap();
    assert_eq!(project_blocked.status, StoredAutomationStatus::Active);
    assert_eq!(project_blocked.config.health_state, "blocked");
    assert_eq!(
        project_blocked.config.blocked_code.as_deref(),
        Some("project_missing")
    );
    assert!(project_blocked.config.target_conversation_id.is_none());
    assert!(project_blocked.config.next_run_at.is_none());
    assert!(service
        .load_conversation("conversation-graph-referenced-project")
        .unwrap()
        .is_none());

    let connection = service.state.connection().unwrap();
    let violations = connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn conversation_fork_clones_exact_history_archives_and_rewrites_trace_refs() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(ChatConversationRecord {
            id: "conversation-archive-source".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "archive source".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-archive-source".to_string(),
                    role: "user".to_string(),
                    content: "read it".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-archive-source".to_string(),
                    role: "assistant".to_string(),
                    content: "done".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let archive_provider_call_id = "provider-call-archive-source";
    let archive_call_id = crate::llm::model_response_tool_call_id(
        "run-archive-source",
        0,
        0,
        archive_provider_call_id,
    );
    let history_provider_call_id = "provider-call-history-source";
    let history_call_id = crate::llm::model_response_tool_call_id(
        "run-archive-source",
        0,
        1,
        history_provider_call_id,
    );
    let managed_provider_call_id = "provider-call-managed-source";
    let managed_call_id = crate::llm::model_response_tool_call_id(
        "run-archive-source",
        0,
        2,
        managed_provider_call_id,
    );
    let exact = "{\"content\":\"fork exact history\"}".repeat(20_000);
    let archive = service
        .archive_conversation_tool_result(
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                conversation_id: "conversation-archive-source".to_string(),
                assistant_message_id: "assistant-archive-source".to_string(),
                sequence: 1,
                call_id: archive_call_id.clone(),
                tool: "read_file".to_string(),
                content_type: "application/json".to_string(),
                content: exact.clone(),
                truncated_at_source: false,
                model_projection_truncated: false,
                archive_projection_truncated: false,
                created_at: 3,
            },
        )
        .unwrap();
    let managed_session_id = "cmd_000000000000000000000000000000a1";
    let managed_running_exact =
        format!(r#"{{"status":"running","sessionId":"{managed_session_id}"}}"#);
    let managed_running_archive = service
        .archive_conversation_tool_result(
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                conversation_id: "conversation-archive-source".to_string(),
                assistant_message_id: "assistant-archive-source".to_string(),
                sequence: 5,
                call_id: managed_call_id.clone(),
                tool: "run_command".to_string(),
                content_type: "application/json".to_string(),
                content: managed_running_exact.clone(),
                truncated_at_source: false,
                model_projection_truncated: false,
                archive_projection_truncated: false,
                created_at: 4,
            },
        )
        .unwrap();
    let managed_terminal_exact =
        r#"{"status":"exited","exitCode":0,"stdout":"managed exact output"}"#.to_string();
    let managed_terminal_archive = service
        .archive_conversation_tool_result(
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                conversation_id: "conversation-archive-source".to_string(),
                assistant_message_id: "assistant-archive-source".to_string(),
                sequence: 6,
                call_id: format!("command-session:{managed_session_id}"),
                tool: "run_command".to_string(),
                content_type: "application/json".to_string(),
                content: managed_terminal_exact.clone(),
                truncated_at_source: false,
                model_projection_truncated: true,
                archive_projection_truncated: false,
                created_at: 5,
            },
        )
        .unwrap();
    let source_archive_open =
        crate::storage::conversation_history_open::encode_archive_history_open(
            archive.archive_ref.clone(),
            37,
        )
        .unwrap();
    let source_tool_exchange_open =
        crate::storage::conversation_history_open::encode_history_open(
            &crate::storage::conversation_history_open::HistoryOpenRoute::ToolExchange {
                reference: crate::storage::conversation_history_repository::ConversationHistoryRecordRef::TraceItem {
                    assistant_message_id: "assistant-archive-source".to_string(),
                    sequence: 1,
                },
            },
        )
        .unwrap();
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-archive-source".to_string(),
        conversation_id: "conversation-archive-source".to_string(),
        assistant_message_id: "assistant-archive-source".to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: true,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: archive_call_id.clone(),
                tool: "read_file".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "read_file".to_string(),
                },
                operation: serde_json::json!({ "path": "large.txt" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: archive_call_id.clone(),
                tool: "read_file".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "path": "large.txt", "summary": "bounded" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: true,
                archive: crate::conversation_trace::ConversationHistoryArchiveTraceMetadata {
                    archive_ref: Some(archive.archive_ref.clone()),
                    content_hash: Some(archive.content_hash.clone()),
                    archived_bytes: Some(archive.total_bytes),
                    archived_completely: Some(true),
                    history_projection_truncated: true,
                    ..Default::default()
                },
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 2,
                call_id: history_call_id.clone(),
                tool: "conversation_history".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "conversation_history".to_string(),
                },
                operation: serde_json::json!({ "open": source_archive_open }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 3,
                call_id: history_call_id.clone(),
                tool: "conversation_history".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({
                    "view": "exact_tool_result",
                    "navigation": {
                        "next": source_archive_open,
                        "toolExchange": source_tool_exchange_open
                    }
                }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 4,
                call_id: managed_call_id.clone(),
                tool: "run_command".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "run_command".to_string(),
                },
                operation: serde_json::json!({ "command": "python3 app.py" }),
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 5,
                call_id: managed_call_id.clone(),
                tool: "run_command".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({
                    "status": "running",
                    "sessionId": managed_session_id
                }),
                approval_status: crate::AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: crate::conversation_trace::ConversationHistoryArchiveTraceMetadata {
                    archive_ref: Some(managed_running_archive.archive_ref.clone()),
                    content_hash: Some(managed_running_archive.content_hash.clone()),
                    archived_bytes: Some(managed_running_archive.total_bytes),
                    archived_completely: Some(true),
                    ..Default::default()
                },
            },
            ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: 6,
                phase: crate::ConversationCommandSessionLifecyclePhase::Terminal,
                session_id: managed_session_id.to_string(),
                call_id: managed_call_id.clone(),
                status: crate::AgentCommandSessionStatus::Exited,
                exit_code: Some(0),
                latest_sequence: 1,
                output_truncated: false,
                archive: crate::conversation_trace::ConversationHistoryArchiveTraceMetadata {
                    archive_ref: Some(managed_terminal_archive.archive_ref.clone()),
                    content_hash: Some(managed_terminal_archive.content_hash.clone()),
                    archived_bytes: Some(managed_terminal_archive.total_bytes),
                    archived_completely: Some(true),
                    model_projection_truncated: true,
                    ..Default::default()
                },
                created_at: 5,
            },
        ],
    };
    let exact_model_items = vec![
        crate::ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: archive_call_id.clone(),
                name: "read_file".to_string(),
                args: serde_json::json!({ "path": "large.txt" }),
                provider_identity: crate::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: archive_provider_call_id.to_string(),
                    runtime_call_id: archive_call_id.clone(),
                },
            }],
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 1,
            ordinal: 0,
            role: "tool".to_string(),
            content: r#"{"ok":true,"result":{"content":"EXACT_FORK_MODEL_MARKER"}}"#.to_string(),
            tool_call_id: Some(archive_call_id.clone()),
            tool_calls: Vec::new(),
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 2,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: history_call_id.clone(),
                name: "conversation_history".to_string(),
                args: serde_json::json!({ "open": source_archive_open }),
                provider_identity: crate::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: history_provider_call_id.to_string(),
                    runtime_call_id: history_call_id.clone(),
                },
            }],
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 3,
            ordinal: 0,
            role: "tool".to_string(),
            content: serde_json::json!({
                "view": "exact_tool_result",
                "navigation": {
                    "next": source_archive_open,
                    "toolExchange": source_tool_exchange_open
                }
            })
            .to_string(),
            tool_call_id: Some(history_call_id.clone()),
            tool_calls: Vec::new(),
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 4,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: managed_call_id.clone(),
                name: "run_command".to_string(),
                args: serde_json::json!({ "command": "python3 app.py" }),
                provider_identity: crate::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: managed_provider_call_id.to_string(),
                    runtime_call_id: managed_call_id.clone(),
                },
            }],
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 5,
            ordinal: 0,
            role: "tool".to_string(),
            content: managed_running_exact.clone(),
            tool_call_id: Some(managed_call_id.clone()),
            tool_calls: Vec::new(),
            is_error: false,
        },
    ];
    let mut in_progress_trace = trace.clone();
    in_progress_trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::InProgress;
    service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &in_progress_trace,
            &exact_model_items,
            2,
            3,
        )
        .unwrap();
    service
        .replace_conversation_turn_trace(&trace, 2, 4)
        .unwrap();

    let forked = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-archive-request",
            "conversation-archive-source",
            "assistant-archive-source",
        ))
        .unwrap()
        .conversation;
    let forked_assistant = forked.messages.last().unwrap();
    let forked_trace = service
        .get_conversation_turn_trace(&forked_assistant.id)
        .unwrap()
        .unwrap();
    let forked_model_context = service
        .get_conversation_model_context_log(&forked_assistant.id)
        .unwrap()
        .unwrap();
    assert_eq!(forked_model_context.items.len(), exact_model_items.len());
    assert!(forked_model_context.items[1]
        .content
        .contains("EXACT_FORK_MODEL_MARKER"));
    let ConversationTurnTraceItem::ToolResult {
        archive: forked_archive,
        ..
    } = &forked_trace.items[1]
    else {
        panic!("forked trace must retain the result");
    };
    assert_ne!(
        forked_archive.archive_ref.as_deref(),
        Some(archive.archive_ref.as_str())
    );
    assert_eq!(
        forked_archive.content_hash.as_deref(),
        Some(archive.content_hash.as_str())
    );
    let forked_archive_ref = forked_archive.archive_ref.as_deref().unwrap();
    let ConversationTurnTraceItem::ToolResult {
        observation: forked_history_observation,
        ..
    } = &forked_trace.items[3]
    else {
        panic!("forked trace must retain the conversation_history result");
    };
    let forked_trace_next = forked_history_observation["navigation"]["next"]
        .as_str()
        .unwrap();
    assert_eq!(
        crate::storage::conversation_history_open::decode_history_open(forked_trace_next).unwrap(),
        crate::storage::conversation_history_open::HistoryOpenRoute::Archive {
            archive_ref: forked_archive_ref.to_string(),
            start_char: 37,
        }
    );
    let forked_trace_source = forked_history_observation["navigation"]["toolExchange"]
        .as_str()
        .unwrap();
    assert_eq!(
        crate::storage::conversation_history_open::decode_history_open(forked_trace_source)
            .unwrap(),
        crate::storage::conversation_history_open::HistoryOpenRoute::ToolExchange {
            reference: crate::storage::conversation_history_repository::ConversationHistoryRecordRef::TraceItem {
                assistant_message_id: forked_assistant.id.clone(),
                sequence: 1,
            },
        }
    );
    let forked_model_history =
        serde_json::from_str::<serde_json::Value>(&forked_model_context.items[3].content).unwrap();
    assert_eq!(
        crate::storage::conversation_history_open::decode_history_open(
            forked_model_history["navigation"]["next"].as_str().unwrap()
        )
        .unwrap(),
        crate::storage::conversation_history_open::HistoryOpenRoute::Archive {
            archive_ref: forked_archive_ref.to_string(),
            start_char: 37,
        }
    );
    assert!(
        !forked_model_context.items[3]
            .content
            .contains(&archive.archive_ref),
        "forked model history must not retain the source archive identity inside an opaque open"
    );
    let page = service
        .read_conversation_history_archive_page(
            &forked.id,
            forked_archive_ref,
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
            0,
            u64::MAX,
        )
        .unwrap()
        .unwrap();
    assert_eq!(page.content, exact);
    let hits = service
        .search_conversation_history(
            &forked.id,
            "fork exact history",
            &crate::storage::conversation_history_repository::ConversationHistorySearchFilter {
                include_archives: true,
                tool: Some("read_file".to_string()),
                ..Default::default()
            },
            10,
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(matches!(
        &hits[0].reference,
        crate::storage::conversation_history_repository::ConversationHistoryRecordRef::Archive {
            archive_ref
        } if Some(archive_ref.as_str()) == forked_archive.archive_ref.as_deref()
    ));

    let ConversationTurnTraceItem::ToolResult {
        archive: forked_managed_running,
        ..
    } = &forked_trace.items[5]
    else {
        panic!("forked trace must retain the managed running result");
    };
    let ConversationTurnTraceItem::CommandSessionLifecycle {
        archive: forked_managed_terminal,
        ..
    } = &forked_trace.items[6]
    else {
        panic!("forked trace must retain the managed terminal lifecycle");
    };
    let forked_managed_running_ref = forked_managed_running.archive_ref.as_deref().unwrap();
    let forked_managed_terminal_ref = forked_managed_terminal.archive_ref.as_deref().unwrap();
    assert_ne!(forked_managed_running_ref, forked_managed_terminal_ref);
    assert_eq!(
        forked_managed_running.content_hash.as_deref(),
        Some(managed_running_archive.content_hash.as_str())
    );
    assert_eq!(
        forked_managed_terminal.content_hash.as_deref(),
        Some(managed_terminal_archive.content_hash.as_str())
    );
    for (archive_ref, expected_call_id, expected_content) in [
        (
            forked_managed_running_ref,
            managed_call_id.as_str(),
            managed_running_exact.as_str(),
        ),
        (
            forked_managed_terminal_ref,
            "command-session:cmd_000000000000000000000000000000a1",
            managed_terminal_exact.as_str(),
        ),
    ] {
        let descriptor = {
            let connection = service.state.connection().unwrap();
            crate::storage::conversation_history_archive_repository::find_archive_by_ref(
                &connection,
                &forked.id,
                archive_ref,
            )
            .unwrap()
            .unwrap()
        };
        assert_eq!(descriptor.call_id, expected_call_id);
        let page = service
            .read_conversation_history_archive_page(
                &forked.id,
                archive_ref,
                crate::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
                0,
                u64::MAX,
            )
            .unwrap()
            .unwrap();
        assert_eq!(page.content, expected_content);
    }

    let recursively_forked = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-archive-recursive-request",
            forked.id.clone(),
            forked_assistant.id.clone(),
        ))
        .unwrap()
        .conversation;
    let recursive_assistant = recursively_forked.messages.last().unwrap();
    let recursive_trace = service
        .get_conversation_turn_trace(&recursive_assistant.id)
        .unwrap()
        .unwrap();
    let recursive_refs = [&recursive_trace.items[5], &recursive_trace.items[6]]
        .into_iter()
        .map(|item| match item {
            ConversationTurnTraceItem::ToolResult { archive, .. }
            | ConversationTurnTraceItem::CommandSessionLifecycle { archive, .. } => {
                archive.archive_ref.as_deref().unwrap()
            }
            _ => panic!("recursive managed archive item has the wrong type"),
        })
        .collect::<Vec<_>>();
    assert_ne!(recursive_refs[0], recursive_refs[1]);
    let connection = service.state.connection().unwrap();
    let recursive_call_ids = recursive_refs
        .iter()
        .map(|archive_ref| {
            crate::storage::conversation_history_archive_repository::find_archive_by_ref(
                &connection,
                &recursively_forked.id,
                archive_ref,
            )
            .unwrap()
            .unwrap()
            .call_id
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recursive_call_ids,
        vec![
            managed_call_id,
            "command-session:cmd_000000000000000000000000000000a1".to_string(),
        ]
    );
}

#[test]
fn conversation_fork_blocks_source_wide_active_command_without_mutating_it() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    save_forkable_command_conversation(&service, "conversation-active-fork");

    let active_session_id = "cmd_000000000000000000000000000000b1";
    create_fork_test_command_session(
        &service,
        active_session_id,
        "conversation-active-fork",
        "assistant-active-fork-2",
        10,
    );
    let running_session_id = "cmd_000000000000000000000000000000b2";
    create_fork_test_command_session(
        &service,
        running_session_id,
        "conversation-active-fork",
        "assistant-active-fork-2",
        10,
    );
    service
        .mark_agent_command_session_running("conversation-active-fork", running_session_id, 11)
        .unwrap();
    let starting_before = service
        .load_agent_command_session("conversation-active-fork", active_session_id)
        .unwrap()
        .unwrap();
    let running_before = service
        .load_agent_command_session("conversation-active-fork", running_session_id)
        .unwrap()
        .unwrap();

    let error = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-active-command-request",
            "conversation-active-fork",
            // The active command belongs to a later turn. Fork admission is intentionally scoped
            // to the complete source conversation, not only the copied prefix.
            "assistant-active-fork-1",
        ))
        .unwrap_err();
    assert_eq!(
        error,
        crate::storage::conversation_fork_repository::ConversationForkError::ActiveCommandSession {
            conversation_id: "conversation-active-fork".to_string(),
            active_session_count: 2,
        }
    );
    let starting_after = service
        .load_agent_command_session("conversation-active-fork", active_session_id)
        .unwrap()
        .unwrap();
    let running_after = service
        .load_agent_command_session("conversation-active-fork", running_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        starting_after.snapshot.status,
        crate::AgentCommandSessionStatus::Starting
    );
    assert_eq!(starting_after.updated_at, starting_before.updated_at);
    assert_eq!(starting_after.settled_at, starting_before.settled_at);
    assert_eq!(
        running_after.snapshot.status,
        crate::AgentCommandSessionStatus::Running
    );
    assert_eq!(running_after.updated_at, running_before.updated_at);
    assert_eq!(running_after.settled_at, running_before.settled_at);

    settle_fork_test_command_session(
        &service,
        active_session_id,
        "conversation-active-fork",
        crate::AgentCommandSessionStatus::Exited,
        12,
    );
    settle_fork_test_command_session(
        &service,
        running_session_id,
        "conversation-active-fork",
        crate::AgentCommandSessionStatus::Interrupted,
        12,
    );
    for (suffix, status) in [
        ('3', crate::AgentCommandSessionStatus::TimedOut),
        ('4', crate::AgentCommandSessionStatus::Failed),
    ] {
        let session_id = format!("cmd_000000000000000000000000000000b{suffix}");
        create_fork_test_command_session(
            &service,
            &session_id,
            "conversation-active-fork",
            "assistant-active-fork-2",
            20,
        );
        settle_fork_test_command_session(
            &service,
            &session_id,
            "conversation-active-fork",
            status,
            21,
        );
    }
    let outcome_unknown_session_id = "cmd_000000000000000000000000000000b5";
    create_fork_test_command_session(
        &service,
        outcome_unknown_session_id,
        "conversation-active-fork",
        "assistant-active-fork-2",
        30,
    );
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "UPDATE agent_command_sessions
                 SET status = 'outcome_unknown', ended_at = 31,
                     terminal_reason = 'restart', updated_at = 31, settled_at = 31
                 WHERE session_id = ?1",
                [outcome_unknown_session_id],
            )
            .unwrap();
    }

    let forked = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-after-command-settlement",
            "conversation-active-fork",
            "assistant-active-fork-1",
        ))
        .unwrap()
        .conversation;
    assert_eq!(forked.messages.len(), 2);

    create_fork_test_command_session(
        &service,
        "cmd_000000000000000000000000000000b6",
        "conversation-active-fork",
        "assistant-active-fork-2",
        50,
    );
    let idempotent_retry = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-after-command-settlement",
            "conversation-active-fork",
            "assistant-active-fork-1",
        ))
        .unwrap()
        .conversation;
    assert_eq!(idempotent_retry.id, forked.id);
}

#[test]
fn agent_tree_fork_rejects_an_active_member_command_without_partial_target_state() {
    const SOURCE_CONVERSATION_ID: &str = "conversation-active-member-tree";
    const SOURCE_ROOT_AGENT_ID: &str = "agent-active-member-tree-root";
    const CHILD_CONVERSATION_ID: &str = "conversation-active-member-tree-child";
    const CHILD_AGENT_ID: &str = "agent-active-member-tree-child";
    const CHILD_ASSISTANT_MESSAGE_ID: &str = "assistant-active-member-tree-child";
    const ACTIVE_SESSION_ID: &str = "cmd_000000000000000000000000000000d1";
    const FORK_REQUEST_ID: &str = "fork-active-member-tree-request";

    let fixture = StorageFixture::new();
    let service = fixture.service();
    let created_at = now_ms();
    let fork_boundary_at = created_at.saturating_add(60_000);
    service
        .save_conversation(ChatConversationRecord {
            id: SOURCE_CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "active member tree".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-active-member-tree-root".to_string(),
                    role: "user".to_string(),
                    content: "start the root task".to_string(),
                    created_at,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-active-member-tree-root".to_string(),
                    role: "assistant".to_string(),
                    content: "fork boundary".to_string(),
                    created_at: fork_boundary_at,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: Some(
                        serde_json::json!({
                            "runId": "run-active-member-tree-root",
                            "status": "completed"
                        })
                        .to_string(),
                    ),
                    ui_state_json: None,
                },
            ],
            created_at,
            updated_at: fork_boundary_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: SOURCE_ROOT_AGENT_ID.to_string(),
            conversation_id: SOURCE_CONVERSATION_ID.to_string(),
            creation_request_id: "ensure-active-member-tree-root".to_string(),
            task_name: "Active member tree".to_string(),
        })
        .unwrap();
    service
        .save_conversation(ChatConversationRecord {
            id: CHILD_CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "active child".to_string(),
            messages: Vec::new(),
            created_at: created_at.saturating_add(1),
            updated_at: created_at.saturating_add(1),
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "INSERT INTO agent_nodes (
                     agent_id, schema_version, root_agent_id, root_conversation_id,
                     parent_agent_id, conversation_id, project_id, creation_request_id,
                     task_name, task_path,
                     model_config_id_snapshot, model_display_name_snapshot,
                     model_supports_image_snapshot, model_context_window_tokens_snapshot,
                     model_settings_revision_snapshot, provider_connection_revision_snapshot,
                     provider_protocol_revision_snapshot, model_selection_source_snapshot,
                     lifecycle, revision, created_at, updated_at
                 ) VALUES (
                     ?1, 1, ?2, ?3, ?2, ?4, NULL, ?5,
                     'active_child', '/root/active_child',
                     'model-1', 'Model 1', 0, 4096,
                     'settings-v1', 'connection-v1', 'protocol-v1', 'explicit',
                     'active', 1, ?6, ?6
                 )",
                rusqlite::params![
                    CHILD_AGENT_ID,
                    SOURCE_ROOT_AGENT_ID,
                    SOURCE_CONVERSATION_ID,
                    CHILD_CONVERSATION_ID,
                    "create-active-member-tree-child",
                    created_at.saturating_add(1),
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, agent_run_json,
                     created_at, position
                 ) VALUES (
                     ?1, ?2, 'assistant', '', 'sent',
                     json_object('runId', 'run-active-member-tree-child',
                                 'status', 'completed'),
                     ?3, 0
                 )",
                rusqlite::params![
                    CHILD_ASSISTANT_MESSAGE_ID,
                    CHILD_CONVERSATION_ID,
                    created_at.saturating_add(2),
                ],
            )
            .unwrap();
    }
    create_fork_test_command_session(
        &service,
        ACTIVE_SESSION_ID,
        CHILD_CONVERSATION_ID,
        CHILD_ASSISTANT_MESSAGE_ID,
        10,
    );

    let durable_fork_state = || {
        let connection = service.state.connection().unwrap();
        let read_ids = |sql: &str| {
            connection
                .prepare(sql)
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        (
            read_ids("SELECT id FROM conversations ORDER BY id"),
            read_ids("SELECT agent_id FROM agent_nodes ORDER BY agent_id"),
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_forks WHERE request_id = ?1",
                    [FORK_REQUEST_ID],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_member_conversation_forks
                     WHERE root_fork_request_id = ?1",
                    [FORK_REQUEST_ID],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
        )
    };
    let before = durable_fork_state();
    assert_eq!(before.2, 0);
    assert_eq!(before.3, 0);
    assert_eq!(
        service.list_agent_tree(SOURCE_ROOT_AGENT_ID).unwrap().len(),
        2
    );

    let error = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            FORK_REQUEST_ID,
            SOURCE_CONVERSATION_ID,
            "assistant-active-member-tree-root",
        ))
        .unwrap_err();
    assert_eq!(
        error,
        crate::storage::conversation_fork_repository::ConversationForkError::ActiveCommandSession {
            conversation_id: CHILD_CONVERSATION_ID.to_string(),
            active_session_count: 1,
        }
    );

    let after = durable_fork_state();
    assert_eq!(
        after, before,
        "a rejected tree fork must leave no target state"
    );
    assert_eq!(
        service
            .load_agent_command_session(CHILD_CONVERSATION_ID, ACTIVE_SESSION_ID)
            .unwrap()
            .unwrap()
            .snapshot
            .status,
        crate::AgentCommandSessionStatus::Starting
    );
    let connection = service.state.connection().unwrap();
    let violations = connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn fork_commit_rechecks_active_commands_before_writing_any_target_state() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    save_forkable_command_conversation(&service, "conversation-fork-race");
    let archive = service
        .archive_conversation_tool_result(
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                conversation_id: "conversation-fork-race".to_string(),
                assistant_message_id: "assistant-fork-race-1".to_string(),
                sequence: 1,
                call_id: "call-fork-race".to_string(),
                tool: "read_file".to_string(),
                content_type: "text/plain".to_string(),
                content: "fork race exact content".to_string(),
                truncated_at_source: false,
                model_projection_truncated: false,
                archive_projection_truncated: false,
                created_at: 3,
            },
        )
        .unwrap();
    service
        .replace_conversation_turn_trace(
            &ConversationTurnTrace {
                schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "run-conversation-fork-race-1".to_string(),
                conversation_id: "conversation-fork-race".to_string(),
                assistant_message_id: "assistant-fork-race-1".to_string(),
                terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: vec![
                    ConversationTurnTraceItem::ToolCall {
                        sequence: 0,
                        call_id: "call-fork-race".to_string(),
                        tool: "read_file".to_string(),
                        provenance: crate::AgentToolIdentity::Builtin {
                            tool_name: "read_file".to_string(),
                        },
                        operation: serde_json::json!({ "path": "large.txt" }),
                        approval_status: crate::AgentApprovalStatus::NotRequired,
                        truncated: false,
                    },
                    ConversationTurnTraceItem::ToolResult {
                        sequence: 1,
                        call_id: "call-fork-race".to_string(),
                        tool: "read_file".to_string(),
                        status: crate::ConversationTraceToolResultStatus::Succeeded,
                        success: true,
                        observation: serde_json::json!({ "content": "bounded" }),
                        approval_status: crate::AgentApprovalStatus::NotRequired,
                        error: None,
                        truncated: true,
                        archive:
                            crate::conversation_trace::ConversationHistoryArchiveTraceMetadata {
                                archive_ref: Some(archive.archive_ref.clone()),
                                content_hash: Some(archive.content_hash.clone()),
                                archived_bytes: Some(archive.total_bytes),
                                archived_completely: Some(true),
                                history_projection_truncated: true,
                                ..Default::default()
                            },
                    },
                ],
            },
            2,
            3,
        )
        .unwrap();

    let mut connection = service.state.connection().unwrap();
    let plan = crate::storage::conversation_fork_repository::build_fork_plan_at_point(
        &connection,
        "fork-race-request",
        "conversation-fork-race",
        &ConversationForkPoint::AssistantReply {
            assistant_message_id: "assistant-fork-race-1".to_string(),
        },
        40,
    )
    .unwrap();
    crate::storage::agent_command_session_repository::create_session(
        &mut connection,
        &fork_test_command_session_create(
            "cmd_000000000000000000000000000000c1",
            "conversation-fork-race",
            "assistant-fork-race-2",
            41,
        ),
    )
    .unwrap();
    let error =
        crate::storage::conversation_fork_repository::commit_fork_plan(&mut connection, &plan)
            .unwrap_err();
    assert!(matches!(
        error,
        crate::storage::conversation_fork_repository::ConversationForkError::ActiveCommandSession {
            active_session_count: 1,
            ..
        }
    ));

    crate::storage::agent_command_session_repository::commit_terminal(
        &mut connection,
        &crate::storage::agent_command_session_repository::AgentCommandSessionTerminalUpdate {
            conversation_id: "conversation-fork-race",
            session_id: "cmd_000000000000000000000000000000c1",
            status: crate::AgentCommandSessionStatus::Failed,
            ended_at: 42,
            exit_code: None,
            latest_sequence: 0,
            transcript_truncated: false,
            output_capture_truncated: false,
            archive_ref: None,
            terminal_reason: Some("test settlement"),
            published_outputs: &[],
            artifact_observation: None,
            committed_at: 42,
        },
    )
    .unwrap();
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER force_fork_commit_failure
             BEFORE INSERT ON conversation_forks
             BEGIN
                 SELECT RAISE(ABORT, 'forced fork commit failure');
             END;",
        )
        .unwrap();
    let forced_error =
        crate::storage::conversation_fork_repository::commit_fork_plan(&mut connection, &plan)
            .unwrap_err();
    assert!(matches!(
        forced_error,
        crate::storage::conversation_fork_repository::ConversationForkError::Other(_)
    ));

    for (table, predicate) in [
        ("conversations", "id = ?1"),
        ("conversation_history_blobs", "conversation_id = ?1"),
        ("conversation_history_blob_chunks", "archive_ref IN (SELECT archive_ref FROM conversation_history_blobs WHERE conversation_id = ?1)"),
        ("conversation_history_fts", "conversation_id = ?1"),
        ("conversation_forks", "target_conversation_id = ?1"),
    ] {
        let count = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {predicate}"),
                [&plan.target.id],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table} retained partial target state");
    }
    for table in [
        "messages",
        "conversation_turn_traces",
        "conversation_turn_trace_items",
        "conversation_model_context_items",
    ] {
        let column = if matches!(table, "messages" | "conversation_turn_traces") {
            "conversation_id"
        } else {
            "assistant_message_id"
        };
        let value = if column == "assistant_message_id" {
            plan.target.messages[1].id.as_str()
        } else {
            plan.target.id.as_str()
        };
        let count = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                [value],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table} retained partial target state");
    }
}

fn save_forkable_command_conversation(service: &StorageService, conversation_id: &str) {
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
                id: format!(
                    "{role}-{conversation_id_suffix}-{suffix}",
                    conversation_id_suffix = conversation_id.trim_start_matches("conversation-")
                ),
                role: role.to_string(),
                content: format!("{role} {suffix}"),
                created_at,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
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

fn fork_test_command_session_create(
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

fn create_fork_test_command_session(
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

fn settle_fork_test_command_session(
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

#[test]
fn conversation_fork_clones_all_visible_turn_diffs_and_supports_recursive_forks() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let source_conversation_id = "conversation-turn-diff-source";
    let messages = [
        ("user-turn-1", "user", 1, None),
        ("assistant-turn-1", "assistant", 2, Some("run-turn-1")),
        ("user-turn-2", "user", 3, None),
        ("assistant-turn-2", "assistant", 4, Some("run-turn-2")),
        ("user-turn-3", "user", 5, None),
        ("assistant-turn-3", "assistant", 6, Some("run-turn-3")),
    ]
    .into_iter()
    .map(|(id, role, created_at, run_id)| ChatMessageRecord {
        id: id.to_string(),
        role: role.to_string(),
        content: format!("content {id}"),
        created_at,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: run_id.map(|run_id| {
            serde_json::json!({
                "runId": run_id,
                "status": "completed"
            })
            .to_string()
        }),
        ui_state_json: None,
    })
    .collect::<Vec<_>>();
    service
        .save_conversation(ChatConversationRecord {
            id: source_conversation_id.to_string(),
            project_id: Some("project-1".to_string()),
            model_id: Some("model-1".to_string()),
            title: "turn diff source".to_string(),
            messages,
            created_at: 1,
            updated_at: 6,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();

    let workspace_root = fixture.root.join("project-1").to_string_lossy().to_string();
    let source_turns = [
        (
            "run-turn-1",
            "assistant-turn-1",
            "action-turn-1",
            AgentTurnFileChange {
                path: "src/first.rs".to_string(),
                before: crate::AgentTurnFileContent::Missing,
                after: crate::AgentTurnFileContent::Text("first\n".to_string()),
            },
        ),
        (
            "run-turn-2",
            "assistant-turn-2",
            "action-turn-2",
            AgentTurnFileChange {
                path: "src/second.rs".to_string(),
                before: crate::AgentTurnFileContent::Text("before\n".to_string()),
                after: crate::AgentTurnFileContent::Text("after\n".to_string()),
            },
        ),
        (
            "run-turn-3",
            "assistant-turn-3",
            "action-turn-3",
            AgentTurnFileChange {
                path: "src/after-cutoff.rs".to_string(),
                before: crate::AgentTurnFileContent::Missing,
                after: crate::AgentTurnFileContent::Text("excluded\n".to_string()),
            },
        ),
    ];
    for (run_id, assistant_message_id, action_id, change) in &source_turns {
        service
            .replace_conversation_turn_trace(
                &ConversationTurnTrace {
                    schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                    run_id: (*run_id).to_string(),
                    conversation_id: source_conversation_id.to_string(),
                    assistant_message_id: (*assistant_message_id).to_string(),
                    terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
                    terminal_error: None,
                    truncated: false,
                    items: Vec::new(),
                },
                1,
                2,
            )
            .unwrap();
        let identity = AgentTurnDiffIdentity {
            run_id: (*run_id).to_string(),
            conversation_id: source_conversation_id.to_string(),
            assistant_message_id: (*assistant_message_id).to_string(),
            project_id: "project-1".to_string(),
            workspace_root: workspace_root.clone(),
        };
        service.initialize_agent_turn_diff(&identity).unwrap();
        assert!(service
            .record_agent_turn_file_change(&identity, action_id, change)
            .unwrap());
    }

    let first_fork = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-turn-diffs-through-second",
            source_conversation_id,
            "assistant-turn-2",
        ))
        .unwrap()
        .conversation;
    assert_eq!(first_fork.messages.len(), 4);
    let forked_assistants = first_fork
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
        .collect::<Vec<_>>();
    let forked_run_id = |message: &ChatMessageRecord| {
        serde_json::from_str::<serde_json::Value>(
            message.agent_run_json.as_deref().expect("agent run"),
        )
        .unwrap()["runId"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let latest = service
        .load_latest_agent_turn_diff(&first_fork.id, "project-1")
        .unwrap()
        .unwrap();
    assert_eq!(
        latest.identity.assistant_message_id,
        forked_assistants[1].id
    );
    assert_eq!(
        latest.files,
        vec![AgentTurnFileChange {
            path: "src/second.rs".to_string(),
            before: crate::AgentTurnFileContent::Text("before\n".to_string()),
            after: crate::AgentTurnFileContent::Text("after\n".to_string()),
        }]
    );
    let forked_turns = service
        .load_agent_turn_diffs_for_messages(
            &first_fork.id,
            "project-1",
            &forked_assistants
                .iter()
                .map(|message| message.id.clone())
                .collect::<Vec<_>>(),
        )
        .unwrap();
    assert_eq!(forked_turns.len(), 2);
    assert_eq!(
        forked_turns[0].identity.assistant_message_id,
        forked_assistants[0].id
    );
    assert_eq!(forked_turns[0].files[0].path, "src/first.rs");
    assert_eq!(
        forked_turns[1].identity.assistant_message_id,
        forked_assistants[1].id
    );
    assert_eq!(forked_turns[1].files[0].path, "src/second.rs");

    for (message, action_id, change) in [
        (forked_assistants[0], "action-turn-1", &source_turns[0].3),
        (forked_assistants[1], "action-turn-2", &source_turns[1].3),
    ] {
        let identity = AgentTurnDiffIdentity {
            run_id: forked_run_id(message),
            conversation_id: first_fork.id.clone(),
            assistant_message_id: message.id.clone(),
            project_id: "project-1".to_string(),
            workspace_root: workspace_root.clone(),
        };
        assert!(
            !service
                .record_agent_turn_file_change(&identity, action_id, change)
                .unwrap(),
            "forked action ids must retain their idempotency evidence"
        );
    }

    let recursive_fork = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-turn-diffs-recursively",
            first_fork.id.clone(),
            forked_assistants[0].id.clone(),
        ))
        .unwrap()
        .conversation;
    assert_eq!(recursive_fork.messages.len(), 2);
    let recursive_latest = service
        .load_latest_agent_turn_diff(&recursive_fork.id, "project-1")
        .unwrap()
        .unwrap();
    assert_eq!(
        recursive_latest.files,
        vec![AgentTurnFileChange {
            path: "src/first.rs".to_string(),
            before: crate::AgentTurnFileContent::Missing,
            after: crate::AgentTurnFileContent::Text("first\n".to_string()),
        }]
    );
}

#[test]
fn composer_drafts_only_preserve_full_for_current_permission_semantics() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    let mut legacy = composer_draft("legacy", None, "legacy full");
    legacy.permission_mode = "full".to_string();
    legacy.permission_mode_version = 0;
    let legacy = service.save_composer_draft(legacy).unwrap();
    assert_eq!(legacy.permission_mode, "default");

    let mut current = composer_draft("current", None, "current full");
    current.permission_mode = "full".to_string();
    current.permission_mode_version =
        crate::storage::models::CURRENT_COMPOSER_PERMISSION_MODE_VERSION;
    current.queued_messages_json = r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide","attachments":[],"modelId":"model-default","permissionMode":"full","projectId":null,"skills":[],"status":"pending","createdAt":1}]"#.to_string();
    let current = service.save_composer_draft(current).unwrap();
    assert_eq!(current.permission_mode, "full");
    assert_eq!(
        current.queued_messages_json,
        r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide","attachments":[],"modelId":"model-default","permissionMode":"full","projectId":null,"skills":[],"status":"pending","createdAt":1}]"#
    );

    let stored = service.load_composer_drafts().unwrap();
    assert_eq!(
        stored
            .iter()
            .find(|draft| draft.scope_id == "legacy")
            .unwrap()
            .permission_mode,
        "default"
    );
    assert_eq!(
        stored
            .iter()
            .find(|draft| draft.scope_id == "current")
            .unwrap()
            .permission_mode,
        "full"
    );
    assert_eq!(
        stored
            .iter()
            .find(|draft| draft.scope_id == "current")
            .unwrap()
            .queued_messages_json,
        r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide","attachments":[],"modelId":"model-default","permissionMode":"full","projectId":null,"skills":[],"status":"pending","createdAt":1}]"#
    );
}

#[test]
fn composer_draft_message_save_preserves_heavy_payloads_and_rejects_stale_text() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    let mut draft = composer_draft("message-only", None, "before");
    draft.updated_at = 10;
    draft.attachments_json = r#"[{"id":"attachment-1","kind":"file","name":"large.bin","sizeBytes":1,"encoding":"base64","data":"YQ=="}]"#.to_string();
    draft.skills_json =
        r#"[{"id":"workspace:workspace:skill-1","revision":"skill-sha256-v1:test"}]"#.to_string();
    service.save_composer_draft(draft.clone()).unwrap();

    assert!(service
        .save_composer_draft_message("message-only", "after", 11)
        .unwrap());
    assert!(service
        .save_composer_draft_message("message-only", "stale", 9)
        .unwrap());
    assert!(!service
        .save_composer_draft_message("missing", "seed me", 1)
        .unwrap());

    let stored = service
        .load_composer_drafts()
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.scope_id == "message-only")
        .unwrap();
    assert_eq!(stored.message, "after");
    assert_eq!(stored.updated_at, 11);
    assert_eq!(stored.attachments_json, draft.attachments_json);
    assert_eq!(stored.skills_json, draft.skills_json);
}

#[test]
fn composer_draft_save_rejects_non_current_nested_payloads() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    let serialized = serde_json::to_value(composer_draft("draft-wire", None, "draft")).unwrap();
    for missing in ["modelId", "projectId"] {
        let mut incomplete = serialized.clone();
        incomplete.as_object_mut().unwrap().remove(missing);
        assert!(serde_json::from_value::<ComposerDraftRecord>(incomplete).is_err());
    }

    let mut missing_queue_fields = composer_draft("draft-missing-queue", None, "draft");
    missing_queue_fields.queued_messages_json =
        r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide"}]"#.to_string();
    assert_eq!(
        service
            .save_composer_draft(missing_queue_fields)
            .unwrap_err(),
        "stored_composer_draft_malformed"
    );

    let mut extra_attachment_field = composer_draft("draft-extra-attachment", None, "draft");
    extra_attachment_field.attachments_json = r#"[{"id":"attachment-1","kind":"file","name":"empty.txt","sizeBytes":0,"encoding":"utf8","data":"","future":true}]"#.to_string();
    assert_eq!(
        service
            .save_composer_draft(extra_attachment_field)
            .unwrap_err(),
        "stored_composer_draft_malformed"
    );

    let mut malformed_skill = composer_draft("draft-malformed-skill", None, "draft");
    malformed_skill.skills_json =
        r#"[{"id":"missing-source-shape","revision":"revision"}]"#.to_string();
    assert_eq!(
        service.save_composer_draft(malformed_skill).unwrap_err(),
        "stored_composer_draft_malformed"
    );
}

#[test]
fn composer_draft_load_rejects_a_corrupt_current_row_instead_of_filtering_items() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let connection = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version, model_id, project_id,
                attachments_json, skills_json, queued_messages_json, updated_at
             ) VALUES (?1, '', 'default', ?2, NULL, NULL, '[]', '[]', '[{}]', 1)",
            rusqlite::params![
                "draft-corrupt-load",
                crate::storage::models::CURRENT_COMPOSER_PERMISSION_MODE_VERSION
            ],
        )
        .unwrap();

    assert_eq!(
        service.load_composer_drafts().unwrap_err(),
        "stored_composer_draft_malformed"
    );
}

#[test]
fn stale_saves_cannot_recreate_deleted_project_data() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let stale_conversation = conversation("conversation-stale", Some("project-1"), "message-stale");
    service
        .save_conversation(stale_conversation.clone())
        .unwrap();
    service.delete_project("project-1").unwrap();

    assert!(service
        .save_conversation(stale_conversation.clone())
        .is_err());
    assert!(service
        .save_conversation_meta(ChatConversationMetaRecord {
            id: stale_conversation.id.clone(),
            project_id: stale_conversation.project_id.clone(),
            model_id: stale_conversation.model_id.clone(),
            title: stale_conversation.title.clone(),
            created_at: stale_conversation.created_at,
            updated_at: stale_conversation.updated_at,
            pinned_at: stale_conversation.pinned_at,
            archived_at: stale_conversation.archived_at,
            unread_at: stale_conversation.unread_at,
        })
        .is_err());
    assert!(service
        .upsert_chat_messages(
            &stale_conversation.id,
            stale_conversation.messages.clone(),
            0,
        )
        .is_err());
    assert!(service
        .save_composer_draft(composer_draft(
            &stale_conversation.id,
            Some("project-1"),
            "stale draft",
        ))
        .is_err());
    assert!(service.load_conversations().unwrap().is_empty());
    assert!(service.load_composer_drafts().unwrap().is_empty());
}

#[test]
fn deleting_conversation_removes_agent_rows_and_keeps_usage_rollup() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-1",
            Some("project-1"),
            "message-1",
        ))
        .unwrap();
    service
        .upsert_agent_usage(agent_usage_record("conversation-1", "message-1"))
        .unwrap();
    service
        .store_pending_agent_action(pending_action("action-1", "conversation-1"))
        .unwrap();
    service
        .upsert_agent_action_audit(action_audit("action-1", "conversation-1"))
        .unwrap();

    service.delete_conversation("conversation-1").unwrap();

    let summary = service
        .get_usage_summary(
            &crate::AgentUsageSummaryInput {
                range: crate::AgentUsageSummaryRange::All,
                from: None,
                to: None,
            },
            200_000,
        )
        .unwrap();
    assert_eq!(summary.request_count, 1);
    assert_eq!(summary.message_count, 1);
    assert_eq!(summary.input_tokens, Some(12));
    assert_eq!(summary.output_tokens, Some(8));
    assert_eq!(summary.total_tokens, Some(20));

    let connection = service.state.connection().unwrap();
    let raw_usage_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_usage_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    let rollup_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_deleted_usage_daily_rollups",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let pending_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_pending_actions", [], |row| {
            row.get(0)
        })
        .unwrap();
    let audit_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_action_audit", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert_eq!(raw_usage_count, 0);
    assert_eq!(rollup_count, 1);
    assert_eq!(pending_count, 0);
    assert_eq!(audit_count, 0);
}

#[test]
fn deleting_messages_keeps_usage_totals_via_rollup() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut record = conversation("conversation-usage-delete", Some("project-1"), "message-1");
    record.messages.push(ChatMessageRecord {
        id: "message-2".to_string(),
        role: "assistant".to_string(),
        content: "done".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    record.updated_at = 2;
    service.save_conversation(record).unwrap();
    let mut first_usage = agent_usage_record("conversation-usage-delete", "message-1");
    first_usage.estimated_cost = Some(0.25);
    service.upsert_agent_usage(first_usage).unwrap();
    let mut second_usage = agent_usage_record("conversation-usage-delete", "message-2");
    second_usage.estimated_cost = Some(0.25);
    service.upsert_agent_usage(second_usage).unwrap();

    service
        .delete_chat_messages("conversation-usage-delete", &["message-1".to_string()])
        .unwrap();

    let summary = service
        .get_usage_summary(
            &crate::AgentUsageSummaryInput {
                range: crate::AgentUsageSummaryRange::All,
                from: None,
                to: None,
            },
            200_000,
        )
        .unwrap();
    assert_eq!(summary.request_count, 2);
    assert_eq!(summary.message_count, 2);
    assert_eq!(summary.input_tokens, Some(24));
    assert_eq!(summary.output_tokens, Some(16));
    assert_eq!(summary.total_tokens, Some(40));
    assert_eq!(summary.estimated_cost, Some(0.5));

    let connection = service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_usage_records", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_deleted_usage_daily_rollups",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}
