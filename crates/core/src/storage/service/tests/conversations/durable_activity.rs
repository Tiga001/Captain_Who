use super::*;

#[test]
fn loading_a_backend_owned_turn_projects_durable_tool_activity_without_renderer_writes() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut stored = conversation("conversation-observer-trace", None, "message-user");
    stored.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: "message-assistant".to_string(),
        role: "assistant".to_string(),
        content: "Done".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
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
fn loading_a_durable_image_generation_result_restores_the_live_renderer_contract() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-image-generation-reload";
    let assistant_message_id = "assistant-image-generation-reload";
    let run_id = "run-image-generation-reload";
    let call_id = "call-image-generation-reload";
    let digest = "a".repeat(64);
    let artifact_uri = format!("image-artifact://sha256/{digest}");
    let operation = serde_json::json!({
        "request": {
            "operation": "generate",
            "prompt": "private prompt"
        },
        "reason": "Create a title image."
    });
    let durable_observation = serde_json::json!({
        "schemaVersion": 1,
        "status": "succeeded",
        "operation": "generate",
        "artifact": {
            "artifactId": format!("sha256:{digest}"),
            "uri": artifact_uri.clone(),
            "kind": "image",
            "format": "jpeg",
            "mimeType": "image/jpeg",
            "width": 1024,
            "height": 768,
            "sizeBytes": 12345,
            "sha256": digest.clone()
        },
        "audit": {
            "executionId": "agent-v1:image-reload",
            "requestFingerprint": format!("sha256:{}", "b".repeat(64)),
            "providerProfileId": "default",
            "adapterId": "smartmlSeedream",
            "profileRevision": 1,
            "modelId": "image-model",
            "createdAt": 3,
            "completedAt": 4,
            "durationMs": 1
        },
        "path": artifact_uri.clone(),
        "savedPath": "/private/generated-image.jpg",
        "visualInputStatus": "unsupportedByCurrentModel",
        "binaryOmittedFromHistory": true
    });
    let mut existing_run: serde_json::Value = serde_json::from_str(
        &crate::storage::chat_repository::canonical_agent_run_lifecycle_projection(
            None,
            run_id,
            "completed",
            2,
            5,
            Some(5),
        )
        .unwrap(),
    )
    .unwrap();
    existing_run["toolCalls"] = serde_json::json!([{
        "id": call_id,
        "tool": "image_generation",
        "args": operation.clone(),
        "approvalStatus": "not_required",
        "reason": null
    }]);
    existing_run["toolResults"] = serde_json::json!([{
        "callId": call_id,
        "tool": "image_generation",
        "ok": true,
        "result": durable_observation.clone()
    }]);
    let mut stored = conversation(conversation_id, None, "message-user");
    stored.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: "Done".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: Some(existing_run.to_string()),
        ui_state_json: None,
    });
    service.save_conversation(stored).unwrap();
    service
        .replace_conversation_turn_trace(
            &ConversationTurnTrace {
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
                        tool: "image_generation".to_string(),
                        provenance: crate::AgentToolIdentity::Builtin {
                            tool_name: "image_generation".to_string(),
                        },
                        operation,
                        approval_status: crate::AgentApprovalStatus::NotRequired,
                        truncated: false,
                    },
                    ConversationTurnTraceItem::ToolResult {
                        sequence: 1,
                        call_id: call_id.to_string(),
                        tool: "image_generation".to_string(),
                        status: crate::ConversationTraceToolResultStatus::Succeeded,
                        success: true,
                        observation: durable_observation,
                        approval_status: crate::AgentApprovalStatus::NotRequired,
                        error: None,
                        truncated: false,
                        archive: Default::default(),
                    },
                ],
            },
            2,
            5,
        )
        .unwrap();

    let loaded = service.load_conversation(conversation_id).unwrap().unwrap();
    let run: serde_json::Value = serde_json::from_str(
        loaded.messages[1]
            .agent_run_json
            .as_deref()
            .expect("completed durable image generation should restore an AgentRun"),
    )
    .unwrap();
    let call = &run["toolCalls"][0];
    assert_eq!(call["reason"], "Create a title image.");
    assert_eq!(call["args"]["request"]["operation"], "generate");
    assert_eq!(call["args"]["request"]["hasInputImage"], false);
    assert!(call["args"]["request"].get("prompt").is_none());

    let result = &run["toolResults"][0]["result"];
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["artifact"]["uri"], artifact_uri);
    for private_field in [
        "path",
        "savedPath",
        "visualInputStatus",
        "binaryOmittedFromHistory",
    ] {
        assert!(result.get(private_field).is_none());
    }
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
    let artifact_observation = current_artifact_observation();
    let stored_run = serde_json::json!({
        "runId": run_id,
        "status": "completed",
        "startedAt": 2,
        "completedAt": 2,
        "toolDefinitions": [],
        "toolCalls": [{
            "id": call_id,
            "tool": "run_command",
            "args": { "command": "render-report" },
            "approvalStatus": "approved",
            "reason": null
        }],
        "toolResults": [],
        "webSearchActivities": [],
        "readActivities": [],
        "approvals": [],
        "fileChangeProposals": [],
        "fileChanges": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {},
        "timeline": [],
        "commandSessions": {
            "call-observer-command": {
                "callId": call_id,
                "status": "exited",
                "startedAt": 2,
                "endedAt": 2,
                "exitCode": 0,
                "latestSequence": 1,
                "outputTruncated": false,
                "artifactObservation": serde_json::to_value(&artifact_observation).unwrap()
            }
        },
        "collaborationTimelineActivities": [{
            "activityId": "activity-command-child",
            "agentId": "agent-command-child",
            "occurredAt": 2,
            "ownerAgentId": "agent-root",
            "ownerConversationId": "conversation-root",
            "taskMessageId": "task-fixture",
            "anchorMessageId": assistant_message_id,
            "traceBoundarySequence": 1,
            "runId": run_id,
            "semantic": "completed",
            "sequence": 1,
            "taskNameSnapshot": "Create report",
            "turnId": null
        }]
    });
    let mut stored = conversation(conversation_id, None, "message-command-user");
    stored.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: "Report ready".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: Some(stored_run.to_string()),
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
                artifact_observation: Some(&artifact_observation),
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
    assert_eq!(
        run["commandSessions"][call_id]["artifactObservation"],
        serde_json::to_value(&artifact_observation).unwrap()
    );
    assert_eq!(run["startedAt"], 2);
    assert_eq!(run["completedAt"], 3);
    assert_eq!(
        run["collaborationTimelineActivities"][0]["agentId"],
        "agent-command-child"
    );
}
