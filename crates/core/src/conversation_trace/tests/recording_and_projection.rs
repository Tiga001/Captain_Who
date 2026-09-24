#[test]
fn binary_fields_are_removed_but_neighboring_text_is_preserved() {
    let result = canonical_tool_result_for_context(&AgentToolResult {
        exact_archive_file: None,
        call_id: "call-1".to_string(),
        tool: "read_image".to_string(),
        ok: true,
        result: Some(json!({
            "path": "image.png",
            "image": { "mimeType": "image/png", "dataBase64": "SGVsbG8=" },
            "note": "keep me",
        })),
        error: None,
    });
    assert_eq!(
        result.result.as_ref().unwrap()["image"]["dataBase64"],
        "[binary/base64 omitted]"
    );
    assert_eq!(result.result.as_ref().unwrap()["note"], "keep me");
}

#[test]
fn binary_sanitization_is_idempotent_and_the_durable_trace_validates() {
    let call = AgentToolCall {
        id: "call-image".to_string(),
        tool: "read_image".to_string(),
        args: json!({ "path": "image.png" }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({
            "path": "image.png",
            "thumbnailDataUrl": "data:image/png;base64,dGh1bWI=",
            "image": {
                "mimeType": "image/png",
                "dataBase64": "ZnVsbC1pbWFnZQ=="
            }
        })),
        error: None,
    };
    let once = canonical_tool_result_for_context(&raw);
    let twice = canonical_tool_result_for_context(&once);

    assert_eq!(once.result, twice.result);
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call);
    recorder.record_tool_result(&call, &once);
    let trace = recorder.finish(
        "run-image",
        "conversation-image",
        "assistant-image",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    assert!(trace.truncated);
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains("data:image/png;base64"));
    assert!(!serialized.contains("ZnVsbC1pbWFnZQ=="));
    assert!(!serialized.contains("dGh1bWI="));
}

#[test]
fn failed_tool_observation_preserves_sanitized_structured_result_for_model() {
    let result = canonical_tool_result_for_context(&AgentToolResult {
        exact_archive_file: None,
        call_id: "call-command".to_string(),
        tool: "run_command".to_string(),
        ok: false,
        result: Some(json!({
            "exitCode": 1,
            "stdout": "partial output\n",
            "stderr": "ModuleNotFoundError: No module named 'openpyxl'\n",
            "timedOut": false,
            "cancelled": false,
            "diagnosticBase64": "c2VjcmV0",
        })),
        error: Some("命令执行失败。".to_string()),
    });

    let observation = render_tool_observation(&result);

    assert!(!observation.contains("\"ok\""));
    assert!(!observation.contains("\"callId\""));
    assert!(observation.contains("\"exitCode\":1"));
    assert!(observation.contains("partial output\\n"));
    assert!(observation.contains("ModuleNotFoundError"));
    assert!(observation.contains("\"timedOut\":false"));
    assert!(observation.contains("\"cancelled\":false"));
    assert!(observation.contains("[binary/base64 omitted]"));
    assert!(!observation.contains("c2VjcmV0"));
    assert!(observation.contains("命令执行失败。"));
}

#[test]
fn failed_tool_observation_without_structured_result_is_still_well_formed() {
    let observation = render_tool_observation(&AgentToolResult {
        exact_archive_file: None,
        call_id: "call-failed-before-execution".to_string(),
        tool: "run_command".to_string(),
        ok: false,
        result: None,
        error: Some("命令未执行。".to_string()),
    });

    let observation: Value = serde_json::from_str(&observation).unwrap();
    assert_eq!(observation["status"], "failed");
    assert_eq!(observation["error"], "命令未执行。");
}

#[test]
fn committed_prefix_never_ends_on_a_tool_call() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("Before approval.").unwrap();
    recorder.record_tool_call(&call("pending"));
    let committed = recorder.snapshot().committed_prefix();
    assert_eq!(committed.items.len(), 1);
    assert!(committed.items[0].is_safe_compaction_boundary());
}

#[test]
fn in_progress_audit_keeps_open_call_while_context_prefix_does_not() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_narration("I will generate the image.")
        .unwrap();
    let mut pending = call("image-call");
    pending.tool = "image_generation".to_string();
    pending.args = json!({
        "request": { "operation": "generate", "prompt": "private prompt" },
        "reason": "Create the requested image."
    });
    recorder.record_tool_call(&pending);
    let snapshot = recorder.snapshot();

    let audit = snapshot.in_progress_audit_trace("run", "conversation", "assistant");
    let context = snapshot.in_progress_trace("run", "conversation", "assistant");
    audit.validate().unwrap();
    context.validate().unwrap();
    assert!(matches!(
        audit.items.last(),
        Some(ConversationTurnTraceItem::ToolCall { call_id, .. }) if call_id == "image-call"
    ));
    assert!(matches!(
        context.items.last(),
        Some(ConversationTurnTraceItem::AssistantNarration { .. })
    ));
    let mut terminal = audit.clone();
    terminal.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
    terminal.terminal_error = Some("interrupted".to_string());
    assert!(terminal.validate().is_err());
}

#[test]
fn recovered_result_closes_the_same_durable_call_once() {
    let mut recorder = ConversationTraceRecorder::default();
    let mut pending = call("image-call");
    pending.tool = "image_generation".to_string();
    pending.args = json!({
        "request": { "operation": "generate", "prompt": "private prompt" },
        "reason": "Create the requested image."
    });
    recorder.record_tool_call(&pending);
    let audit = recorder
        .snapshot()
        .in_progress_audit_trace("run", "conversation", "assistant");
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: pending.id.clone(),
        tool: pending.tool.clone(),
        ok: false,
        result: Some(json!({ "status": "outcomeIndeterminate" })),
        error: Some("execution interrupted".to_string()),
    };

    let recovered = conversation_trace_with_recovered_tool_result(&audit, &result).unwrap();
    recovered.validate().unwrap();
    assert!(matches!(
        recovered.items.last(),
        Some(ConversationTurnTraceItem::ToolResult { call_id, .. }) if call_id == "image-call"
    ));
    assert!(conversation_trace_with_recovered_tool_result(&recovered, &result).is_err());
}

#[test]
fn user_guidance_is_canonical_ordered_and_never_persists_attachment_bytes() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("Initial answer.").unwrap();
    let sequence = recorder
        .record_user_guidance(
            "guidance-1",
            "client-1",
            "Please also inspect the image.",
            &[AgentInputAttachment {
                id: "attachment-1".to_string(),
                kind: AgentInputAttachmentKind::Image,
                name: "diagram.png".to_string(),
                mime_type: Some("image/png".to_string()),
                size_bytes: 6,
                encoding: crate::protocol::AgentInputAttachmentEncoding::Base64,
                data: "c2VjcmV0".to_string(),
                content_sha256: None,
                truncated: None,
            }],
            &[],
            42,
        )
        .unwrap();
    assert_eq!(sequence, 1);

    let trace = recorder.finish(
        "run-1",
        "conversation-1",
        "assistant-1",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(serialized.contains("\"type\":\"user_guidance\""));
    assert!(serialized.contains("diagram.png"));
    assert!(!serialized.contains("c2VjcmV0"));
}

#[test]
fn attachment_only_user_guidance_is_recorded_without_fake_user_text() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("Initial answer.").unwrap();
    recorder
        .record_user_guidance(
            "guidance-attachment",
            "client-attachment",
            "",
            &[AgentInputAttachment {
                id: "attachment-1".to_string(),
                kind: AgentInputAttachmentKind::File,
                name: "notes.txt".to_string(),
                mime_type: Some("text/plain".to_string()),
                size_bytes: 5,
                encoding: crate::protocol::AgentInputAttachmentEncoding::Managed,
                data: String::new(),
                content_sha256: Some("hash".to_string()),
                truncated: None,
            }],
            &[],
            42,
        )
        .unwrap();

    let trace = recorder.finish(
        "run-1",
        "conversation-1",
        "assistant-1",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    let guidance = trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::UserGuidance { content, .. } => Some(content),
            _ => None,
        })
        .expect("attachment-only guidance is retained");
    assert!(guidance.is_empty());
}

#[test]
fn current_trace_attachment_rejects_unknown_fields() {
    assert!(
        serde_json::from_value::<ConversationTraceAttachment>(json!({
            "id": "attachment-1",
            "kind": "image",
            "name": "diagram.png",
            "sizeBytes": 6,
            "futureField": true
        }))
        .is_err()
    );
}

#[test]
fn user_guidance_cannot_split_an_open_tool_exchange() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call("pending"));
    assert!(recorder
        .record_user_guidance("guidance-1", "client-1", "change course", &[], &[], 42)
        .is_none());
}

#[test]
fn staged_and_direct_file_change_projection_keeps_effect_metadata_without_payloads() {
    let staged_call = AgentToolCall {
        id: "staged-1".into(),
        tool: "apply_patch".into(),
        args: json!({
            "request": {
                "action": "append",
                "transactionId": "file-change-1",
                "index": 0,
                "expectedDraftRevision": 0,
                "content": "first\nsecond\nSECRET_WRITE_BODY"
            }
        }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let patch_call = AgentToolCall {
        id: "patch-1".into(),
        tool: "apply_patch".into(),
        args: json!({
            "request": {
                "action": "apply",
                "operation": "update",
                "filePath": "src/lib.rs",
                "observationId": "fobs_trace_projection",
                "edits": [{
                    "kind": "replace",
                    "oldText": "old",
                    "newText": "new\nextra"
                }],
                "summary": "Update the implementation"
            }
        }),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&staged_call);
    recorder.record_tool_result(
        &staged_call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: staged_call.id.clone(),
            tool: staged_call.tool.clone(),
            ok: true,
            result: Some(json!({
                "status": "drafting",
                "operation": "create",
                "transactionId": "file-change-1",
                "filePath": "notes.txt",
                "additions": 3,
                "deletions": 0,
                "lineCount": 3,
                "byteCount": 30,
                "privateTail": "SECRET_WRITE_BODY"
            })),
            error: None,
        },
    );
    recorder.record_tool_call(&patch_call);
    recorder.record_tool_result(
        &patch_call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: patch_call.id.clone(),
            tool: patch_call.tool.clone(),
            ok: true,
            result: Some(json!({
                "schemaVersion": 1,
                "status": "applied",
                "outcome": "applied",
                "transactionId": "file-change-direct-1",
                "operation": "update",
                "updateStrategy": null,
                "filePath": "src/lib.rs",
                "additions": 2,
                "deletions": 1,
                "lineCount": 2,
                "byteCount": 9,
                "revision": "file-revision-sha256-v1:fixture",
                "errorCode": null,
                "error": null,
                "message": "文件已更新。"
            })),
            error: None,
        },
    );

    let trace = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains("SECRET_WRITE_BODY"));
    assert!(!serialized.contains("--- a/src/lib.rs"));

    let ConversationTurnTraceItem::ToolCall {
        operation: staged_operation,
        ..
    } = &trace.items[0]
    else {
        panic!("expected staged call");
    };
    assert_eq!(staged_operation["request"]["contentBytes"], 30);
    assert_eq!(staged_operation["request"]["additions"], 3);
    let ConversationTurnTraceItem::ToolResult {
        observation: staged_result,
        ..
    } = &trace.items[1]
    else {
        panic!("expected staged result");
    };
    assert_eq!(staged_result["filePath"], "notes.txt");
    assert_eq!(staged_result["additions"], 3);

    let ConversationTurnTraceItem::ToolCall {
        operation: patch_operation,
        ..
    } = &trace.items[2]
    else {
        panic!("expected patch call");
    };
    assert_eq!(patch_operation["request"]["filePath"], "src/lib.rs");
    assert_eq!(patch_operation["request"]["additions"], 2);
    assert_eq!(patch_operation["request"]["deletions"], 1);
    let ConversationTurnTraceItem::ToolResult {
        observation: patch_result,
        approval_status,
        ..
    } = &trace.items[3]
    else {
        panic!("expected patch result");
    };
    assert_eq!(*approval_status, AgentApprovalStatus::Approved);
    assert_eq!(patch_result["status"], "applied");
    assert_eq!(patch_result["additions"], 2);
    assert_eq!(patch_result["deletions"], 1);
    assert!(patch_result.get("gitDiff").is_none());
    assert!(patch_result.get("content").is_none());
}

#[test]
fn command_and_read_results_keep_bounded_tail_or_summary() {
    let command_call = AgentToolCall {
        id: "command-1".into(),
        tool: "run_command".into(),
        args: json!({
            "command": "cargo test",
            "cwd": "workspace",
            "reason": "verify"
        }),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    };
    let read_call = AgentToolCall {
        id: "read-1".into(),
        tool: "read_file".into(),
        args: json!({ "path": "src/lib.rs", "startLine": 20, "maxLines": 50 }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&command_call);
    recorder.record_tool_result(
        &command_call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: command_call.id.clone(),
            tool: command_call.tool.clone(),
            ok: false,
            result: Some(json!({
                "command": "cargo test",
                "cwd": "workspace",
                "exitCode": 1,
                "stdout": format!("BEGIN_MUST_NOT_SURVIVE{}", "x".repeat(4_000)),
                "stderr": "the actionable failure is at the end",
                "timedOut": false,
                "cancelled": false,
                "durationMs": 25,
                "stdoutTruncated": false,
                "stderrTruncated": false
            })),
            error: Some("command failed".into()),
        },
    );
    recorder.record_tool_call(&read_call);
    recorder.record_tool_result(
        &read_call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: read_call.id.clone(),
            tool: read_call.tool.clone(),
            ok: true,
            result: Some(json!({
                "path": "src/lib.rs",
                "startLine": 20,
                "endLine": 200,
                "totalLines": 500,
                "truncated": true,
                "content": format!("READ_PREFIX{}", "z".repeat(5_000))
            })),
            error: None,
        },
    );
    let trace = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains("BEGIN_MUST_NOT_SURVIVE"));
    assert!(!serialized.contains(&"z".repeat(2_000)));

    let ConversationTurnTraceItem::ToolResult {
        observation: command,
        ..
    } = &trace.items[1]
    else {
        panic!("expected command result");
    };
    assert_eq!(command["command"], "cargo test");
    assert_eq!(command["cwd"], "workspace");
    assert_eq!(command["exitCode"], 1);
    assert!(command["stdoutTail"]
        .as_str()
        .unwrap()
        .starts_with("[earlier output omitted]"));
    assert!(command["stderrTail"]
        .as_str()
        .unwrap()
        .contains("actionable failure"));

    let ConversationTurnTraceItem::ToolCall {
        operation: read_operation,
        ..
    } = &trace.items[2]
    else {
        panic!("expected read call");
    };
    assert_eq!(read_operation["path"], "src/lib.rs");
    assert_eq!(read_operation["startLine"], 20);
    let ConversationTurnTraceItem::ToolResult {
        observation: read, ..
    } = &trace.items[3]
    else {
        panic!("expected read result");
    };
    assert_eq!(read["path"], "src/lib.rs");
    assert_eq!(read["startLine"], 20);
    assert_eq!(read["contentTruncatedInHistory"], true);
    assert!(read["summary"].as_str().unwrap().contains("READ_PREFIX"));
}

#[test]
fn durable_projection_redacts_embedded_data_urls_and_hidden_reasoning() {
    let call = AgentToolCall {
        id: "unknown-1".into(),
        tool: "extension_tool".into(),
        args: json!({
            "note": "prefix data:image/png;base64,U0VDUkVUX0lNQUdF suffix",
            "nested": { "arbitraryBase64": "U0VDUkVU", "safe": "keep" }
        }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call);
    recorder.record_tool_result(
        &call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({
                "message": "before data:image/jpeg;base64,QUJDRA== after",
                "reasoning": "hidden chain must not persist",
                "safe": "visible evidence"
            })),
            error: None,
        },
    );
    let trace = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains("U0VDUkVUX0lNQUdF"));
    assert!(!serialized.contains("U0VDUkVU"));
    assert!(!serialized.contains("QUJDRA=="));
    assert!(!serialized.contains("hidden chain must not persist"));
    assert!(serialized.contains("visible evidence"));
    trace.validate().unwrap();
}

#[test]
fn repeated_unchanged_read_failure_keeps_paired_reference_instead_of_duplicate_error() {
    let mut recorder = ConversationTraceRecorder::default();
    for id in ["read-1", "read-2"] {
        let call = AgentToolCall {
            id: id.into(),
            tool: "read_file".into(),
            args: json!({ "path": "missing.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        recorder.record_tool_call(&call);
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: Some(json!({ "path": "missing.txt", "code": "notFound" })),
                error: Some("file does not exist".into()),
            },
        );
    }
    let trace = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Failed,
        Some("unable to read the requested file"),
    );
    trace.validate().unwrap();
    assert_eq!(trace.items.len(), 4);
    let ConversationTurnTraceItem::ToolResult {
        observation,
        error,
        truncated,
        ..
    } = &trace.items[3]
    else {
        panic!("expected repeated result");
    };
    assert_eq!(observation["repeatedFailure"], true);
    assert_eq!(observation["sameAsCallId"], "read-1");
    assert!(error.is_none());
    assert!(*truncated);
}

#[test]
fn approval_checkpoint_keeps_exact_direct_args_while_durable_snapshot_is_bounded() {
    let call = AgentToolCall {
        id: "patch-approval".into(),
        tool: "apply_patch".into(),
        args: json!({
            "request": {
                "action": "apply",
                "operation": "update",
                "filePath": "src/main.rs",
                "observationId": "fobs_approval_checkpoint",
                "edits": [{
                    "kind": "replace",
                    "oldText": "old",
                    "newText": "new"
                }]
            }
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call);
    let (checkpoint, _, _, _) = recorder.checkpoint();
    let ConversationTurnTraceItem::ToolCall {
        operation: checkpoint_operation,
        ..
    } = &checkpoint[0]
    else {
        panic!("expected checkpoint call");
    };
    assert_eq!(
        checkpoint_operation["request"]["observationId"],
        "fobs_approval_checkpoint"
    );
    assert_eq!(
        checkpoint_operation["request"]["edits"][0]["oldText"],
        "old"
    );
    assert_eq!(
        checkpoint_operation["request"]["edits"][0]["newText"],
        "new"
    );

    let durable = recorder
        .snapshot()
        .in_progress_audit_trace("run", "conversation", "assistant");
    let ConversationTurnTraceItem::ToolCall {
        operation: durable_operation,
        ..
    } = &durable.items[0]
    else {
        panic!("expected durable call");
    };
    let durable_request = &durable_operation["request"];
    assert!(durable_request.get("edits").is_none());
    assert!(durable_request.get("observationId").is_none());
    assert_eq!(durable_request["filePath"], "src/main.rs");
    assert_eq!(durable_request["editCount"], 1);
    assert_eq!(durable_request["additions"], 1);
    assert_eq!(durable_request["deletions"], 1);
}

#[test]
fn collaboration_receipt_can_cover_multiple_fifo_messages_with_exact_model_envelopes() {
    let mut recorder = ConversationTraceRecorder::default();
    let first = recorder
        .record_agent_mailbox_delivery(
            0,
            "receipt-1",
            "message-1",
            "agent-parent",
            "Parent",
            "/root",
            crate::AgentMailboxKind::Followup,
            "  first payload  ",
            1,
        )
        .unwrap()
        .unwrap();
    let second = recorder
        .record_agent_mailbox_delivery(
            1,
            "receipt-1",
            "message-2",
            "agent-child",
            "Child",
            "/root/child",
            crate::AgentMailboxKind::Message,
            "second\u{0007}payload",
            2,
        )
        .unwrap()
        .unwrap();
    let snapshot = recorder.snapshot();
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.model_context_items.len(), 2);
    assert_eq!(snapshot.model_context_items[0].content, first);
    assert_eq!(snapshot.model_context_items[1].content, second);
    assert_eq!(
        serde_json::from_str::<Value>(&first).unwrap()["payload"],
        "first payload"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&second).unwrap()["origin"],
        "agent"
    );
}

#[test]
fn collaboration_model_envelope_preserves_long_utf8_text() {
    let payload = "协作🙂".repeat(160_000);
    let first = project_agent_mailbox_model_envelope(
        "agent-parent",
        "Parent",
        "/root",
        crate::AgentMailboxKind::Followup,
        &payload,
    )
    .unwrap();
    let second = project_agent_mailbox_model_envelope(
        "agent-parent",
        "Parent",
        "/root",
        crate::AgentMailboxKind::Followup,
        &payload,
    )
    .unwrap();
    assert_eq!(first, second);
    assert!(!first.1);
    let envelope: Value = serde_json::from_str(&first.0).unwrap();
    assert_eq!(envelope["payloadTruncated"], false);
    assert_eq!(envelope["payload"], payload);
}

#[test]
fn collaboration_result_envelope_exposes_only_authenticated_semantic_state() {
    let result = crate::AgentTurnResultEnvelope {
        schema_version: crate::AGENT_RESULT_ENVELOPE_SCHEMA_VERSION,
        child_agent_id: "private-child".into(),
        task_name: "reviewer".into(),
        task_path: "/root/private-path".into(),
        wake_id: "private-wake".into(),
        turn_id: Some("private-turn".into()),
        run_id: Some("private-run".into()),
        status: crate::AgentWakeStatus::Completed,
        summary: "Review finished with evidence.".into(),
        artifact_refs: vec![crate::AgentResultArtifactReference {
            artifact_id: "report-artifact".into(),
            kind: crate::AgentResultArtifactKind::Document,
            media_type: "application/pdf".into(),
        }],
        terminal_error: None,
    };
    let payload = serde_json::to_string(&result).unwrap();
    let (projected, truncated) = project_agent_mailbox_model_envelope(
        &result.child_agent_id,
        &result.task_name,
        &result.task_path,
        crate::AgentMailboxKind::Result,
        &payload,
    )
    .unwrap();
    assert!(!truncated);
    assert!(!projected.contains("private-"));
    let envelope: Value = serde_json::from_str(&projected).unwrap();
    assert_eq!(envelope["senderTaskName"], "reviewer");
    assert!(envelope.get("senderAgentId").is_none());
    assert!(envelope.get("senderTaskPath").is_none());
    let model_result: Value = serde_json::from_str(envelope["payload"].as_str().unwrap()).unwrap();
    assert_eq!(
        model_result,
        json!({
            "taskName":"reviewer", "status":"completed", "summary":result.summary,
            "artifacts":result.artifact_refs, "terminalError":null,
        })
    );
    assert!(project_agent_mailbox_model_envelope(
        "another-private-child",
        &result.task_name,
        &result.task_path,
        crate::AgentMailboxKind::Result,
        &payload,
    )
    .is_err());
    assert!(project_agent_mailbox_model_envelope(
        &result.child_agent_id,
        &result.task_name,
        &result.task_path,
        crate::AgentMailboxKind::Result,
        "untyped legacy result",
    )
    .is_err());
    let ordinary = project_agent_mailbox_model_envelope(
        &result.child_agent_id,
        &result.task_name,
        &result.task_path,
        crate::AgentMailboxKind::Message,
        &payload,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&ordinary.0).unwrap()["payload"],
        payload,
        "ordinary message prose must not be parsed as Host-authored identity metadata"
    );
}

#[test]
fn large_collaboration_delivery_precommit_is_an_exact_terminal_trace_prefix() {
    let mut recorder = ConversationTraceRecorder::default();
    let model_content = recorder
        .record_agent_mailbox_delivery(
            0,
            "receipt-large",
            "message-large",
            "agent-child",
            "Child",
            "/root/child",
            crate::AgentMailboxKind::Message,
            &"evidence ".repeat(3_000),
            1,
        )
        .unwrap()
        .unwrap();
    let precommitted = recorder
        .snapshot()
        .in_progress_trace("run", "conversation", "assistant");
    let terminal = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );

    let ConversationTurnTraceItem::AgentMailboxDelivery {
        content: trace_content,
        truncated,
        ..
    } = &precommitted.items[0]
    else {
        panic!("expected Agent mailbox delivery");
    };
    assert_eq!(&model_content, trace_content);
    assert!(!*truncated);
    assert_eq!(
        serde_json::from_str::<Value>(trace_content).unwrap()["payloadTruncated"],
        false
    );
    assert_eq!(
        recorder.snapshot().model_context_items[0].content,
        model_content
    );
    assert_eq!(precommitted.items, terminal.items);
    assert!(!precommitted.truncated);
}
