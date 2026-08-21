use super::*;
use crate::storage::models::AgentUsageRecordInsert;
use crate::storage::{conversation_trace_repository, migrations, usage_repository};
use crate::{
    ConversationTurnTrace, ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};

fn approval_is_safe(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(current_persisted_approval_is_safe)
}

fn current_browser_tool_identity(
    tool_id: &str,
    raw_name: &str,
    model_name: &str,
) -> serde_json::Value {
    serde_json::to_value(crate::AgentToolIdentity::BuiltinCapability {
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
    })
    .unwrap()
}

fn current_browser_agent_run() -> serde_json::Value {
    let evaluate_identity =
        current_browser_tool_identity("browser.evaluate", "browser_evaluate", "browser_evaluate");
    let tabs_identity =
        current_browser_tool_identity("browser.tabs", "browser_tabs", "browser_tabs");
    serde_json::json!({
        "runId": "run-1",
        "status": "running",
        "startedAt": 2,
        "toolDefinitions": [],
        "toolCalls": [
            {
                "id": "call-evaluate",
                "tool": "browser_evaluate",
                "args": {},
                "approvalStatus": "approved",
                "reason": null
            },
            {
                "id": "call-tabs",
                "tool": "browser_tabs",
                "args": {},
                "approvalStatus": "not_required",
                "reason": null
            }
        ],
        "toolResults": [],
        "approvals": [],
        "diffs": [],
        "fileDrafts": [],
        "webSearchActivities": [],
        "readActivities": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {},
        "timeline": [
            {
                "id": "tool-call-call-evaluate",
                "type": "tool_call",
                "callId": "call-evaluate",
                "identity": evaluate_identity,
                "traceSequence": 4
            },
            {
                "id": "tool-call-call-tabs",
                "type": "tool_call",
                "callId": "call-tabs",
                "identity": tabs_identity,
                "traceSequence": 6
            }
        ]
    })
}

fn current_office_approval() -> serde_json::Value {
    serde_json::json!({
        "type": "office_operation",
        "officeOperation": {
            "schemaVersion": crate::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
            "id": "office-call",
            "semanticArgs": {},
            "prepared": {
                "schemaVersion": crate::office::OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
                "providerId": "officecli",
                "engineRevision": "officecli-v1",
                "workspaceRevision": null,
                "access": "fileWrite",
                "request": {
                    "documentKind": "document",
                    "operation": "create",
                    "documentPath": null,
                    "outputPath": null,
                    "destinationPath": null,
                    "inputs": [],
                    "timeoutMs": null,
                    "parameters": { "type": "create" }
                },
                "argv": [],
                "resolvedRenderPlan": null,
                "paths": [],
                "inputBindings": []
            },
            "approvalStatus": "required",
            "reason": "Create a document"
        }
    })
}

fn current_presentation_render_approval() -> serde_json::Value {
    let mut approval = current_office_approval();
    let prepared = &mut approval["officeOperation"]["prepared"];
    prepared["access"] = "readOnly".into();
    prepared["request"]["documentKind"] = "presentation".into();
    prepared["request"]["operation"] = "view".into();
    prepared["request"]["parameters"] = serde_json::json!({
        "type": "view",
        "mode": "screenshot"
    });
    prepared["resolvedRenderPlan"] = serde_json::json!({
        "requestedPages": [1, 2, 3, 4, 5, 6, 7, 8, 9],
        "slideWidthEmu": 12_192_000,
        "slideHeightEmu": 6_858_000,
        "viewport": { "width": 1600, "height": 922 },
        "grid": { "mode": "columns", "columns": 3 }
    });
    approval
}

fn current_skill_installation_approval() -> serde_json::Value {
    serde_json::json!({
        "type": "skill_installation",
        "installation": {
            "schemaVersion": 1,
            "id": "install-call",
            "installRef": "opaque-install-ref",
            "preview": {
                "name": "Example",
                "description": "Example Skill",
                "sourceSummary": {},
                "resolvedRevision": "revision-1",
                "fileCount": 1,
                "totalBytes": 10,
                "resourceSummary": {
                    "total": 1,
                    "references": 0,
                    "assets": 0,
                    "scripts": 0,
                    "bytes": 10
                },
                "containsScripts": false,
                "warnings": [],
                "compatibility": "compatible",
                "operation": "install",
                "impact": "Adds one Skill"
            },
            "approvalStatus": "required",
            "expiresAt": 100
        }
    })
}

#[test]
fn current_persisted_approval_variants_match_the_renderer_projection() {
    let approvals = [
        serde_json::json!({
            "type": "tool_call",
            "call": {
                "id": "tool-call",
                "tool": "read_file",
                "args": {"path": "README.md"},
                "approvalStatus": "required",
                "reason": null
            }
        }),
        serde_json::json!({
            "type": "diff",
            "diff": {
                "id": "diff-call",
                "operation": "update",
                "filePath": "README.md",
                "patch": "@@ -1 +1 @@",
                "baseRevision": null,
                "summary": "Update README",
                "approvalStatus": "required"
            }
        }),
        serde_json::json!({
            "type": "file_write",
            "fileWrite": {
                "id": "write-call",
                "draftId": "draft-1",
                "mode": "rewrite",
                "filePath": "README.md",
                "baseRevision": null,
                "summary": "Rewrite README",
                "additions": 1,
                "deletions": 1,
                "lineCount": 1,
                "byteCount": 16,
                "approvalStatus": "required"
            }
        }),
        serde_json::json!({
            "type": "command",
            "command": {
                "id": "command-call",
                "command": "cargo check",
                "cwd": null,
                "timeoutMs": 30_000,
                "approvalStatus": "required",
                "riskLevel": "read_only",
                "reason": "Check the workspace",
                "observe": {
                    "kinds": ["office"],
                    "expectedOutputs": [""],
                    "additionalRoots": []
                }
            }
        }),
        serde_json::json!({
            "type": "skill_materialization",
            "materialization": {
                "id": "materialize-call",
                "sourceUri": "skill://example/reference.md",
                "sourcePrefix": null,
                "destination": "reference.md",
                "approvalStatus": "required",
                "reason": null
            }
        }),
        serde_json::json!({
            "type": "skill_script",
            "script": {
                "id": "script-call",
                "scriptUri": "skill://example/scripts/check.py",
                "skillId": "workspace:example",
                "skillRevision": "revision-1",
                "resourcePath": "scripts/check.py",
                "resourceDigest": "digest-1",
                "interpreter": "python3",
                "args": ["--check"],
                "requirements": {
                    "pythonDistributions": ["openpyxl"],
                    "commands": []
                },
                "preflight": {
                    "status": "ready",
                    "interpreter": "python3",
                    "interpreterVersion": "3.12",
                    "dependencies": [{
                        "kind": "python_distribution",
                        "name": "openpyxl",
                        "status": "available",
                        "version": "3.1"
                    }],
                    "runtimeFingerprint": "python3:3.12"
                },
                "timeoutMs": null,
                "approvalStatus": "required",
                "reason": "Run the checked script"
            }
        }),
        current_office_approval(),
        current_presentation_render_approval(),
        current_skill_installation_approval(),
    ];

    for approval in approvals {
        assert!(approval_is_safe(&approval), "current approval: {approval}");
    }
    assert!(!approval_is_safe(&serde_json::json!({
        "type": "mcp_tool_call",
        "approval": {}
    })));
}

#[test]
fn persisted_approval_rejects_wrong_office_versions_and_malformed_nested_data() {
    let mut wrong_outer = current_office_approval();
    wrong_outer["officeOperation"]["schemaVersion"] = 4.into();
    assert!(!approval_is_safe(&wrong_outer));

    let mut wrong_inner = current_office_approval();
    wrong_inner["officeOperation"]["prepared"]["schemaVersion"] = 4.into();
    assert!(!approval_is_safe(&wrong_inner));

    let mut missing_render_plan = current_presentation_render_approval();
    missing_render_plan["officeOperation"]["prepared"]["resolvedRenderPlan"] =
        serde_json::Value::Null;
    assert!(!approval_is_safe(&missing_render_plan));

    let mut unresolved_grid = current_presentation_render_approval();
    unresolved_grid["officeOperation"]["prepared"]["resolvedRenderPlan"]["grid"] =
        serde_json::json!({ "mode": "auto" });
    assert!(!approval_is_safe(&unresolved_grid));

    let mut duplicate_pages = current_presentation_render_approval();
    duplicate_pages["officeOperation"]["prepared"]["resolvedRenderPlan"]["requestedPages"] =
        serde_json::json!([1, 2, 2]);
    assert!(!approval_is_safe(&duplicate_pages));

    let mut unexpected_render_plan = current_office_approval();
    unexpected_render_plan["officeOperation"]["prepared"]["resolvedRenderPlan"] =
        current_presentation_render_approval()["officeOperation"]["prepared"]["resolvedRenderPlan"]
            .clone();
    assert!(!approval_is_safe(&unexpected_render_plan));

    let mut unsafe_integer = current_office_approval();
    unsafe_integer["officeOperation"]["prepared"]["request"]["timeoutMs"] =
        (MAX_JS_SAFE_INTEGER + 1).into();
    assert!(!approval_is_safe(&unsafe_integer));

    let mut extra_nested_field = current_office_approval();
    extra_nested_field["officeOperation"]["prepared"]["request"]["parameters"]["unrecognized"] =
        true.into();
    assert!(!approval_is_safe(&extra_nested_field));

    let mut non_object_semantic_args = current_office_approval();
    non_object_semantic_args["officeOperation"]["semanticArgs"] = "create".into();
    assert!(!approval_is_safe(&non_object_semantic_args));

    let mut bad_script_nested_field = serde_json::json!({
        "type": "skill_script",
        "script": {
            "id": "script-call",
            "scriptUri": "skill://example/scripts/check.py",
            "skillId": "workspace:example",
            "skillRevision": "revision-1",
            "resourcePath": "scripts/check.py",
            "resourceDigest": "digest-1",
            "interpreter": "python3",
            "args": [],
            "requirements": {},
            "preflight": {
                "status": "ready",
                "interpreter": "python3",
                "runtimeFingerprint": "python3:3.12"
            },
            "timeoutMs": null,
            "approvalStatus": "required",
            "reason": null
        }
    });
    bad_script_nested_field["script"]["preflight"]["unrecognized"] = true.into();
    assert!(!approval_is_safe(&bad_script_nested_field));

    let mut oversized_installation = current_skill_installation_approval();
    oversized_installation["installation"]["preview"]["name"] = "x".repeat(1_025).into();
    assert!(!approval_is_safe(&oversized_installation));
}

#[test]
fn conversation_metadata_listing_does_not_require_message_hydration() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let conversation = conversation();
    save_conversation(&mut connection, conversation.clone()).unwrap();

    let metas = list_conversation_metas(&connection).unwrap();

    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].id, conversation.id);
    assert_eq!(metas[0].title, conversation.title);
}

#[test]
fn full_conversation_save_preserves_retained_trace_and_deletes_missing_trace() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let mut conversation = conversation();
    save_conversation(&mut connection, conversation.clone()).unwrap();

    let stored = get_conversation(&connection, "conversation-1")
        .unwrap()
        .unwrap();
    assert_eq!(stored.messages.len(), 2);
    assert!(get_conversation(&connection, "missing").unwrap().is_none());

    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-1".to_string(),
        conversation_id: conversation.id.clone(),
        assistant_message_id: "assistant-1".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: "Inspecting the workspace.".to_string(),
            truncated: false,
        }],
    };
    conversation_trace_repository::replace_trace(&mut connection, &trace, 10, 20).unwrap();

    conversation.title = "Updated title".to_string();
    conversation.messages[1].content = "Updated final answer".to_string();
    conversation
        .messages
        .push(message("user-2", "user", "follow-up", 3, Some("sent")));
    save_conversation(&mut connection, conversation.clone()).unwrap();

    assert_eq!(
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-1").unwrap(),
        Some(trace.clone())
    );
    let stored = list_conversations(&connection).unwrap();
    assert_eq!(stored[0].messages.len(), 3);
    assert_eq!(stored[0].messages[1].content, "Updated final answer");

    conversation.messages[1].role = "user".to_string();
    save_conversation(&mut connection, conversation.clone()).unwrap();
    assert!(
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-1")
            .unwrap()
            .is_none()
    );
    conversation.messages[1].role = "assistant".to_string();
    save_conversation(&mut connection, conversation.clone()).unwrap();
    conversation_trace_repository::replace_trace(&mut connection, &trace, 10, 20).unwrap();

    conversation
        .messages
        .retain(|message| message.id != "assistant-1");
    save_conversation(&mut connection, conversation).unwrap();
    assert!(
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-1")
            .unwrap()
            .is_none()
    );
}

#[test]
fn caller_owned_message_deletion_can_be_rolled_back_as_one_transaction() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    save_conversation(&mut connection, conversation()).unwrap();

    let transaction = connection.transaction().unwrap();
    delete_messages_in_transaction(&transaction, "conversation-1", &["assistant-1".to_string()])
        .unwrap();
    assert_eq!(
        get_conversation(&transaction, "conversation-1")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        1
    );
    transaction.rollback().unwrap();

    let stored = get_conversation(&connection, "conversation-1")
        .unwrap()
        .unwrap();
    assert_eq!(stored.messages.len(), 2);
    assert_eq!(stored.messages[1].id, "assistant-1");
    assert_eq!(stored.messages[1].role, "assistant");
}

#[test]
fn authoritative_usage_repairs_loaded_json_and_rejects_stale_renderer_usage() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let mut stored = conversation();
    stored.messages[1].agent_run_json = Some(
        serde_json::json!({
            "runId": "run-1",
            "status": "completed",
            "usage": {
                "inputTokens": 92_510,
                "outputTokens": 391,
                "totalTokens": 92_901,
                "billableRequestCount": 1
            },
            "timeline": [{ "id": "presentation-only" }]
        })
        .to_string(),
    );
    save_conversation(&mut connection, stored).unwrap();
    usage_repository::upsert_usage_record(
        &connection,
        &AgentUsageRecordInsert {
            id: "usage-run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            message_id: "assistant-1".to_string(),
            run_id: "run-1".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            started_at: Some(1),
            completed_at: Some(2),
            status: Some("completed".to_string()),
            error: None,
            created_at: 2,
            input_tokens: Some(2_942_988),
            output_tokens: Some(38_253),
            output_thinking_tokens: Some(32_402),
            total_tokens: Some(2_981_241),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: 42,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            estimated_cost: None,
        },
    )
    .unwrap();

    let loaded = get_conversation(&connection, "conversation-1")
        .unwrap()
        .unwrap();
    let run: serde_json::Value =
        serde_json::from_str(loaded.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["usage"]["inputTokens"], 2_942_988);
    assert_eq!(run["usage"]["outputTokens"], 38_253);
    assert_eq!(run["usage"]["outputThinkingTokens"], 32_402);
    assert_eq!(run["usage"]["totalTokens"], 2_981_241);
    assert_eq!(run["usage"]["billableRequestCount"], 42);
    assert_eq!(run["timeline"][0]["id"], "presentation-only");

    update_message_state(
        &connection,
        "conversation-1",
        &ChatMessageStateRecord {
            id: "assistant-1".to_string(),
            content: "final answer".to_string(),
            status: Some("sent".to_string()),
            agent_run_json: Some(
                serde_json::json!({
                    "runId": "run-1",
                    "status": "completed",
                    "usage": {
                        "inputTokens": 92_510,
                        "outputTokens": 391,
                        "totalTokens": 92_901,
                        "billableRequestCount": 1
                    },
                    "timeline": [{ "id": "still-preserved" }]
                })
                .to_string(),
            ),
            ui_state_json: None,
        },
    )
    .unwrap();
    let raw: String = connection
        .query_row(
            "SELECT agent_run_json FROM messages WHERE id = 'assistant-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let run: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(run["usage"]["inputTokens"], 2_942_988);
    assert_eq!(run["usage"]["outputTokens"], 38_253);
    assert_eq!(run["usage"]["billableRequestCount"], 42);
    assert_eq!(run["timeline"][0]["id"], "still-preserved");
}

#[test]
fn ui_state_update_never_rewrites_canonical_message_or_agent_run() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let mut stored = conversation();
    stored.messages[1].content = "canonical final answer".to_string();
    stored.messages[1].status = Some("sent".to_string());
    stored.messages[1].agent_run_json = Some(
        serde_json::json!({
            "runId": "run-1",
            "status": "completed",
            "timeline": [{ "id": "terminal-answer", "type": "message" }]
        })
        .to_string(),
    );
    save_conversation(&mut connection, stored).unwrap();

    update_message_ui_state(
        &connection,
        "conversation-1",
        "assistant-1",
        Some(r#"{"timelineCollapsed":false}"#),
    )
    .unwrap();

    let (content, status, agent_run_json, ui_state_json): (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = connection
        .query_row(
            "SELECT content, status, agent_run_json, ui_state_json
                 FROM messages WHERE id = 'assistant-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(content, "canonical final answer");
    assert_eq!(status.as_deref(), Some("sent"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(agent_run_json.as_deref().unwrap()).unwrap()
            ["timeline"][0]["id"],
        "terminal-answer"
    );
    assert_eq!(
        ui_state_json.as_deref(),
        Some(r#"{"timelineCollapsed":false}"#)
    );
}

#[test]
fn normal_terminal_update_refreshes_conversation_but_startup_repair_does_not() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let mut stored = conversation();
    stored.messages[1].agent_run_json = Some(
        serde_json::json!({
            "runId": "run-1",
            "status": "running",
            "state": { "status": "running", "activeRunId": "run-1" }
        })
        .to_string(),
    );
    save_conversation(&mut connection, stored).unwrap();

    update_message_run_terminal_state(
        &connection,
        "conversation-1",
        "assistant-1",
        "run-1",
        Some("sent"),
        "completed",
        50,
    )
    .unwrap();
    let updated_at: i64 = connection
        .query_row(
            "SELECT updated_at FROM conversations WHERE id = 'conversation-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(updated_at, 50);

    reconcile_message_run_terminal_state(
        &connection,
        "conversation-1",
        "assistant-1",
        "run-1",
        "error",
        "failed",
        100,
    )
    .unwrap();
    let updated_at: i64 = connection
        .query_row(
            "SELECT updated_at FROM conversations WHERE id = 'conversation-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(updated_at, 50);
}

fn assert_complete_current_agent_run_lifecycle(
    run: &serde_json::Value,
    expected_status: &str,
    expected_active_run_id: Option<&str>,
    expected_completed_at: Option<i64>,
) {
    assert_eq!(run["runId"], "run-1");
    assert_eq!(run["status"], expected_status);
    assert_eq!(run["startedAt"], 2);
    match expected_completed_at {
        Some(completed_at) => assert_eq!(run["completedAt"], completed_at),
        None => assert!(run.get("completedAt").is_none()),
    }
    for field in [
        "toolDefinitions",
        "toolCalls",
        "toolResults",
        "approvals",
        "diffs",
        "fileDrafts",
        "webSearchActivities",
        "readActivities",
        "mcpInvocations",
        "timeline",
    ] {
        assert_eq!(run[field], serde_json::json!([]), "current {field}");
    }
    assert_eq!(run["messageStreamCheckpoints"], serde_json::json!({}));
    assert_eq!(run["state"]["status"], expected_status);
    assert_eq!(
        run["state"]["activeRunId"],
        expected_active_run_id
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null)
    );
    assert!(run["state"]["lastError"].is_null());
    assert!(run["state"]["updatedAt"].is_i64());
    assert!(run.get("assistantMessageId").is_none());
    assert!(run.get("fileWritePreviews").is_none());
}

#[test]
fn startup_recovery_writes_complete_current_agent_run_from_missing_or_malformed_json() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    save_conversation(&mut connection, conversation()).unwrap();

    reconcile_message_run_terminal_state(
        &connection,
        "conversation-1",
        "assistant-1",
        "run-1",
        "error",
        "failed",
        100,
    )
    .unwrap();
    let terminal_raw: String = connection
        .query_row(
            "SELECT agent_run_json FROM messages WHERE id = 'assistant-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let terminal: serde_json::Value = serde_json::from_str(&terminal_raw).unwrap();
    assert_complete_current_agent_run_lifecycle(&terminal, "failed", None, Some(100));

    connection
        .execute(
            "UPDATE messages SET agent_run_json = '\"malformed-shape\"' WHERE id = 'assistant-1'",
            [],
        )
        .unwrap();
    update_message_run_waiting_state(&connection, "conversation-1", "assistant-1", "run-1", 200)
        .unwrap();
    let waiting_raw: String = connection
        .query_row(
            "SELECT agent_run_json FROM messages WHERE id = 'assistant-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let waiting: serde_json::Value = serde_json::from_str(&waiting_raw).unwrap();
    assert_complete_current_agent_run_lifecycle(
        &waiting,
        "waiting_for_approval",
        Some("run-1"),
        None,
    );
    assert_eq!(waiting["state"]["updatedAt"], 200);
}

#[test]
fn live_save_and_lifecycle_load_preserve_browser_tool_timeline_identity() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    save_conversation(&mut connection, conversation()).unwrap();

    update_message_state(
        &connection,
        "conversation-1",
        &ChatMessageStateRecord {
            id: "assistant-1".to_string(),
            content: "working".to_string(),
            status: Some("pending".to_string()),
            agent_run_json: Some(current_browser_agent_run().to_string()),
            ui_state_json: None,
        },
    )
    .unwrap();
    update_message_run_waiting_state(&connection, "conversation-1", "assistant-1", "run-1", 10)
        .unwrap();

    let loaded = get_conversation(&connection, "conversation-1")
        .unwrap()
        .unwrap();
    let run: serde_json::Value =
        serde_json::from_str(loaded.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "waiting_for_approval");
    for (index, (call_id, tool_id, tool_name, sequence)) in [
        ("call-evaluate", "browser.evaluate", "browser_evaluate", 4),
        ("call-tabs", "browser.tabs", "browser_tabs", 6),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(run["timeline"][index]["callId"], call_id);
        assert_eq!(
            run["timeline"][index]["identity"]["type"],
            "builtin_capability"
        );
        assert_eq!(run["timeline"][index]["identity"]["modelName"], tool_name);
        assert_eq!(run["timeline"][index]["identity"]["rawName"], tool_name);
        assert_eq!(run["timeline"][index]["identity"]["toolId"], tool_id);
        assert_eq!(run["timeline"][index]["traceSequence"], sequence);
    }
}

#[test]
fn lifecycle_projection_rejects_hidden_mismatched_or_malformed_browser_identity_fields() {
    let valid = current_browser_agent_run();
    let invalid_projections = [
        {
            let mut invalid = valid.clone();
            invalid["timeline"][0]["identity"]["cdpEndpoint"] = "must-not-survive".into();
            invalid
        },
        {
            let mut invalid = valid.clone();
            invalid["timeline"][0]["identity"]["modelName"] = "browser_tabs".into();
            invalid
        },
        {
            let mut invalid = valid;
            invalid["timeline"][0]["traceSequence"] = (-1).into();
            invalid
        },
    ];

    for invalid in invalid_projections {
        let recovered = canonical_agent_run_lifecycle_projection(
            Some(&invalid.to_string()),
            "run-1",
            "waiting_for_approval",
            2,
            20,
            None,
        )
        .unwrap();
        let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
        assert_eq!(recovered["toolCalls"], serde_json::json!([]));
        assert_eq!(recovered["timeline"], serde_json::json!([]));
    }
}

#[test]
fn startup_recovery_does_not_preserve_unknown_or_malformed_current_projection_fields() {
    let current_base = serde_json::json!({
        "runId": "run-1",
        "status": "running",
        "startedAt": 2,
        "toolDefinitions": [],
        "toolCalls": [],
        "toolResults": [],
        "approvals": [{
            "type": "command",
            "command": {
                "id": "command-call",
                "command": "true",
                "cwd": null,
                "timeoutMs": null,
                "approvalStatus": "approved",
                "riskLevel": "read_only",
                "reason": null,
                "observe": null
            }
        }],
        "diffs": [],
        "fileDrafts": [],
        "webSearchActivities": [{
            "callId": "web-call",
            "query": "current query",
            "provider": "test",
            "status": "completed",
            "sources": [{
                "id": "source-1",
                "title": "Current source",
                "url": "https://example.test/source",
                "displayUrl": "example.test/source",
                "domain": "example.test"
            }],
            "updatedAt": 3
        }],
        "readActivities": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {},
        "timeline": [],
        "state": {
            "status": "running",
            "activeRunId": "run-1",
            "lastError": null,
            "updatedAt": 2
        }
    });

    let mut unknown_top = current_base.clone();
    unknown_top["assistantMessageId"] = "must-not-survive".into();
    let recovered = canonical_agent_run_lifecycle_projection(
        Some(&unknown_top.to_string()),
        "run-1",
        "failed",
        2,
        10,
        Some(10),
    )
    .unwrap();
    let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
    assert!(recovered.get("assistantMessageId").is_none());
    assert_complete_current_agent_run_lifecycle(&recovered, "failed", None, Some(10));

    let mut malformed_nested = current_base;
    malformed_nested["state"]["privateRecoveryField"] = true.into();
    malformed_nested["approvals"] = serde_json::json!([{
        "type": "command",
        "command": { "legacyArguments": ["must-not-survive"] }
    }]);
    let recovered = canonical_agent_run_lifecycle_projection(
        Some(&malformed_nested.to_string()),
        "run-1",
        "waiting_for_approval",
        2,
        20,
        None,
    )
    .unwrap();
    let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
    assert_eq!(recovered["approvals"], serde_json::json!([]));
    assert!(recovered["state"].get("privateRecoveryField").is_none());
    assert_complete_current_agent_run_lifecycle(
        &recovered,
        "waiting_for_approval",
        Some("run-1"),
        None,
    );

    let current_with_presentation = serde_json::json!({
        "runId": "run-1",
        "status": "running",
        "startedAt": 2,
        "toolDefinitions": [{
            "name": "run_command",
            "description": "Run a managed command",
            "inputSchema": {"type": "object"},
            "safety": "requires_approval",
            "requiresWorkspace": true,
            "requiresApproval": true,
            "approvalMode": "always"
        }],
        "toolCalls": [{
            "id": "command-call",
            "tool": "run_command",
            "args": {"command": "true"},
            "approvalStatus": "approved",
            "reason": null
        }],
        "toolResults": [],
        "approvals": [{
            "type": "command",
            "command": {
                "id": "command-call",
                "command": "true",
                "cwd": null,
                "timeoutMs": null,
                "approvalStatus": "approved",
                "riskLevel": "read_only",
                "reason": null,
                "observe": null
            }
        }],
        "diffs": [],
        "fileDrafts": [],
        "webSearchActivities": [{
            "callId": "web-call",
            "query": "current query",
            "provider": "test",
            "status": "completed",
            "sources": [{
                "id": "source-1",
                "title": "Current source",
                "url": "https://example.test/source",
                "displayUrl": "example.test/source",
                "domain": "example.test"
            }],
            "updatedAt": 3
        }],
        "readActivities": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {
            "stream-1": {"baseContentLength": 3, "baseWasThinking": false}
        },
        "timeline": [{"id": "message-1", "type": "message", "content": "kept"}],
        "todo": {
            "revision": 1,
            "items": [{
                "id": "todo-1",
                "title": "finish",
                "status": "completed",
                "createdAt": 2,
                "updatedAt": 3
            }],
            "updatedAt": 3
        },
        "skillInstallations": [{
            "action": {
                "schemaVersion": 1,
                "id": "install-1",
                "installRef": "opaque-install-ref",
                "preview": {
                    "name": "Example",
                    "description": "Example Skill",
                    "sourceSummary": {},
                    "resolvedRevision": "revision-1",
                    "fileCount": 1,
                    "totalBytes": 10,
                    "resourceSummary": {
                        "total": 1,
                        "references": 0,
                        "assets": 0,
                        "scripts": 0,
                        "bytes": 10
                    },
                    "containsScripts": false,
                    "warnings": [],
                    "compatibility": "compatible",
                    "operation": "install",
                    "impact": "Adds one Skill"
                },
                "approvalStatus": "approved",
                "expiresAt": 100
            },
            "status": "installed"
        }],
        "commandSessions": {
            "command-call": {
                "callId": "command-call",
                "status": "exited",
                "startedAt": 2,
                "endedAt": 3,
                "exitCode": 0,
                "latestSequence": 1,
                "outputTruncated": false
            }
        },
        "activatedSkills": [{
            "id": "workspace:example",
            "name": "Example",
            "revision": "revision-1",
            "source": {"kind": "workspace", "id": "workspace:example"}
        }],
        "explicitSkillSelections": [{
            "id": "workspace:example",
            "revision": "revision-1"
        }],
        "state": {
            "status": "running",
            "activeRunId": "run-1",
            "lastError": null,
            "updatedAt": 2
        }
    });
    let valid_presentation = current_with_presentation.clone();
    let recovered = canonical_agent_run_lifecycle_projection(
        Some(&current_with_presentation.to_string()),
        "run-1",
        "failed",
        2,
        30,
        Some(30),
    )
    .unwrap();
    let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
    assert_eq!(recovered["timeline"][0]["content"], "kept");
    assert_eq!(
        recovered["webSearchActivities"][0]["sources"][0]["id"],
        "source-1"
    );
    assert_eq!(recovered["todo"]["revision"], 1);
    assert_eq!(recovered["skillInstallations"][0]["status"], "installed");
    assert_eq!(recovered["approvals"][0]["type"], "command");
    assert_eq!(
        recovered["commandSessions"]["command-call"]["status"],
        "exited"
    );
    assert_eq!(recovered["activatedSkills"][0]["name"], "Example");
    assert_eq!(
        recovered["explicitSkillSelections"][0]["revision"],
        "revision-1"
    );

    let invalid_projections = [
        {
            let mut invalid = valid_presentation.clone();
            invalid["toolDefinitions"][0]
                .as_object_mut()
                .unwrap()
                .remove("inputSchema");
            invalid
        },
        {
            let mut invalid = valid_presentation.clone();
            invalid["timeline"][0]["privateField"] = true.into();
            invalid
        },
        {
            let mut invalid = valid_presentation.clone();
            invalid["messageStreamCheckpoints"]["stream-1"]["privateField"] = true.into();
            invalid
        },
        {
            let mut invalid = valid_presentation.clone();
            invalid["toolCalls"][0]
                .as_object_mut()
                .unwrap()
                .remove("reason");
            invalid
        },
        {
            let mut invalid = valid_presentation;
            invalid["webSearchActivities"][0]["sources"][0]["privateField"] = true.into();
            invalid
        },
        {
            let mut invalid = current_with_presentation;
            invalid["approvals"][0]["command"]["runtimeBinding"] = serde_json::json!({});
            invalid["approvals"][0]["command"]["inputs"] = serde_json::json!([]);
            invalid
        },
    ];
    for invalid in invalid_projections {
        let recovered = canonical_agent_run_lifecycle_projection(
            Some(&invalid.to_string()),
            "run-1",
            "failed",
            2,
            40,
            Some(40),
        )
        .unwrap();
        let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
        assert_eq!(recovered["toolDefinitions"], serde_json::json!([]));
        assert_eq!(recovered["timeline"], serde_json::json!([]));
        assert_eq!(recovered["webSearchActivities"], serde_json::json!([]));
        assert_eq!(recovered["messageStreamCheckpoints"], serde_json::json!({}));
    }

    assert!(!current_uuid_is_safe(
        "00000000-0000-0000-0000-000000000000"
    ));
    assert!(!current_uuid_is_safe(
        "ffffffff-ffff-4fff-0fff-ffffffffffff"
    ));
    assert!(current_uuid_is_safe("550e8400-e29b-41d4-a716-446655440000"));
}

#[test]
fn canonical_recovery_preserves_the_current_pre_start_projection() {
    let pre_start = serde_json::json!({
        "runId": null,
        "status": "starting",
        "startedAt": 2,
        "toolDefinitions": [],
        "toolCalls": [],
        "toolResults": [],
        "approvals": [],
        "diffs": [],
        "fileDrafts": [],
        "webSearchActivities": [],
        "readActivities": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {},
        "timeline": [{"id": "message-1", "type": "message", "content": "kept"}]
    });

    let recovered = canonical_agent_run_lifecycle_projection(
        Some(&pre_start.to_string()),
        "run-1",
        "running",
        2,
        3,
        None,
    )
    .unwrap();
    let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
    assert_eq!(recovered["runId"], "run-1");
    assert_eq!(recovered["status"], "running");
    assert_eq!(recovered["timeline"][0]["content"], "kept");
}

fn conversation() -> ChatConversationRecord {
    ChatConversationRecord {
        id: "conversation-1".to_string(),
        project_id: None,
        model_id: Some("model-1".to_string()),
        title: "Conversation".to_string(),
        messages: vec![
            message("user-1", "user", "request", 1, Some("sent")),
            message("assistant-1", "assistant", "final answer", 2, Some("sent")),
        ],
        created_at: 1,
        updated_at: 2,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    }
}

fn message(
    id: &str,
    role: &str,
    content: &str,
    created_at: i64,
    status: Option<&str>,
) -> ChatMessageRecord {
    ChatMessageRecord {
        id: id.to_string(),
        role: role.to_string(),
        content: content.to_string(),
        created_at,
        status: status.map(ToString::to_string),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    }
}
