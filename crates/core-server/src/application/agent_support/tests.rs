use super::*;
use mycopilot_core::{
    AgentCommandArtifactObservationKind, AgentCommandArtifactObservationRequest,
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimeProfile,
    AgentCommandRuntimeResolvedPackage, AgentContextCheckpointItem, AgentContextCheckpointToolCall,
    AgentMcpToolInvocationEvent, AgentRunCheckpoint, AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
    AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

#[test]
fn shared_mcp_renderer_contract_matches_rust_safe_event_serialization() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../packages/protocol/fixtures/agent-mcp-renderer-contract-v1.json"
    ))
    .expect("shared MCP Renderer contract fixture");
    assert_eq!(fixture["schemaVersion"], 1);

    // The Host-only identity includes the argument digest. The shared Renderer fixture
    // intentionally omits it, so restore that one internal field before deserializing the typed
    // Rust action and exercise the production notification projection.
    let mut internal_action = fixture["approvalRequired"]["action"].clone();
    internal_action["approval"]["identity"]["argumentsDigest"] = Value::String("e".repeat(64));
    let action: AgentProposedAction =
        serde_json::from_value(internal_action).expect("typed internal MCP action");
    let call_id = fixture["approvalRequired"]["action"]["approval"]["identity"]["callId"]
        .as_str()
        .expect("fixture call id")
        .to_string();
    let action_id = fixture["approvalRequired"]["action"]["approval"]["identity"]["actionId"]
        .as_str()
        .expect("fixture action id")
        .to_string();
    let checkpoint = AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-owned".to_string(),
        context_items: Vec::new(),
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call_id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_action_id: Some(action_id),
        file_change_run_grant_ref: None,
        pending_file_observation: None,
        pending_tool_call_id: call_id,
        conversation_trace_items: Vec::new(),
        conversation_model_context_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    };

    let approval = agent_event_notification(AgentEvent::ApprovalRequired {
        run_id: "run-owned".to_string(),
        action: Box::new(action.clone()),
        checkpoint: Box::new(checkpoint),
        segment_usage: None,
    });
    assert_eq!(approval["params"], fixture["approvalRequired"]);

    let invocation: AgentMcpToolInvocationEvent =
        serde_json::from_value(fixture["lifecycle"]["invocation"].clone())
            .expect("typed lifecycle fixture");
    let lifecycle = agent_event_notification(AgentEvent::McpToolInvocationStateChanged {
        run_id: "run-owned".to_string(),
        invocation,
    });
    assert_eq!(lifecycle["params"], fixture["lifecycle"]);

    let done = agent_event_notification(AgentEvent::Done {
        run_id: "run-owned".to_string(),
        success: true,
        status: Some(AgentRunStatus::WaitingForApproval),
        content: Some("External MCP approval is required.".to_string()),
        usage: None,
        finish_reason: None,
        proposed_actions: vec![action],
    });
    assert_eq!(done["params"], fixture["done"]);

    let rendered = serde_json::to_string(&fixture).unwrap();
    for forbidden in [
        "argumentsDigest",
        "rawArguments",
        "rawResult",
        "structuredContent",
        "stderr",
        "payloadRef",
        "ciphertext",
        "\"type\":\"tool_call\"",
        "\"type\":\"tool_result\"",
        "toolDefinitions",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "shared Renderer contract leaked forbidden field `{forbidden}`"
        );
    }
}

#[test]
fn child_observer_notification_adds_exact_identity_without_changing_safe_event_projection() {
    let identity = mycopilot_core::AgentCollaborationIdentity {
        agent_id: "agent-child".to_string(),
        root_agent_id: "agent-root".to_string(),
        root_conversation_id: "conversation-root".to_string(),
        parent_agent_id: "agent-root".to_string(),
        parent_task_name: "root".to_string(),
        parent_task_path: "/root".to_string(),
        conversation_id: "conversation-child".to_string(),
        task_name: "child".to_string(),
        task_path: "/root/child".to_string(),
        source_agent_id: "agent-root".to_string(),
        source_kind: mycopilot_core::AgentMailboxKind::Task,
        source_task_name: "root".to_string(),
        source_task_path: "/root".to_string(),
        source_agent_message_id: "mailbox-task".to_string(),
        entrusted_task: "review".to_string(),
        template_instructions: None,
    };
    let event = AgentEvent::MessageDelta {
        run_id: "run-child".to_string(),
        stream_id: Some("stream-child".to_string()),
        delta: "hello".to_string(),
    };
    let notification =
        child_observer_event_notification(&identity, "run-child", "assistant-child", event.clone());
    assert_eq!(
        notification["method"],
        mycopilot_protocol_rs::AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD
    );
    assert_eq!(notification["params"]["rootAgentId"], "agent-root");
    assert_eq!(notification["params"]["agentId"], "agent-child");
    assert_eq!(
        notification["params"]["conversationId"],
        "conversation-child"
    );
    assert_eq!(notification["params"]["runId"], "run-child");
    assert_eq!(
        notification["params"]["assistantMessageId"],
        "assistant-child"
    );
    assert_eq!(
        notification["params"]["event"],
        agent_event_notification(event)["params"]
    );
}

#[test]
fn child_observer_notification_preserves_previously_dropped_runtime_event_projections() {
    let identity = mycopilot_core::AgentCollaborationIdentity {
        agent_id: "agent-child".to_string(),
        root_agent_id: "agent-root".to_string(),
        root_conversation_id: "conversation-root".to_string(),
        parent_agent_id: "agent-root".to_string(),
        parent_task_name: "root".to_string(),
        parent_task_path: "/root".to_string(),
        conversation_id: "conversation-child".to_string(),
        task_name: "child".to_string(),
        task_path: "/root/child".to_string(),
        source_agent_id: "agent-root".to_string(),
        source_kind: mycopilot_core::AgentMailboxKind::Task,
        source_task_name: "root".to_string(),
        source_task_path: "/root".to_string(),
        source_agent_message_id: "mailbox-task".to_string(),
        entrusted_task: "review".to_string(),
        template_instructions: None,
    };
    let events = vec![
        AgentEvent::Started {
            run_id: "run-child".to_string(),
            tool_definitions: Vec::new(),
        },
        AgentEvent::ToolInputProgress {
            run_id: "run-child".to_string(),
            stream_id: "stream-child".to_string(),
            attempt: 1,
            tool_call_index: 0,
            tool_call_id: Some("call-child".to_string()),
            tool: "read_file".to_string(),
            received_bytes: 42,
        },
        AgentEvent::Error {
            run_id: None,
            trace_sequence: None,
            message: "provider temporarily unavailable".to_string(),
            recoverable: true,
            code: Some("provider_unavailable".to_string()),
            details: None,
        },
    ];

    for event in events {
        let safe_event = agent_event_notification(event.clone())["params"].clone();
        let notification =
            child_observer_event_notification(&identity, "run-child", "assistant-child", event);
        assert_eq!(notification["params"]["runId"], "run-child");
        assert_eq!(notification["params"]["event"], safe_event);
    }
}

#[test]
fn renderer_command_projection_excludes_host_runtime_authority_and_private_input_paths() {
    let private_artifact_path = "/private/managed-artifacts/objects/secret/manual.pdf";
    let artifact_uri =
        "artifact://sha256/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let request = AgentCommandRequest {
        id: "command-runtime-profile-1".to_string(),
        command: "pdfinfo '$MYCOPILOT_INPUT_ROOT/manual.pdf'".to_string(),
        cwd: None,
        timeout_ms: Some(30_000),
        approval_status: mycopilot_core::AgentApprovalStatus::Required,
        risk_level: None,
        reason: Some("inspect the approved PDF".to_string()),
        observe: None,
        inputs: vec![mycopilot_core::AgentFileInputBinding {
            schema_version: mycopilot_core::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
            mount_path: "manual.pdf".to_string(),
            source: mycopilot_core::AgentFileInputRef::GeneratedArtifact {
                uri: artifact_uri.to_string(),
                path: private_artifact_path.to_string(),
            },
            size_bytes: 123,
            sha256: "a".repeat(64),
        }],
        runtime_binding: Some(Box::new(AgentCommandRuntimeBinding {
            schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
            profile: AgentCommandRuntimeProfile::Pdf,
            profile_revision: "artifact-runtime-profile-sha256-v1:test".to_string(),
            provider_id: "mycopilot.artifact-runtime".to_string(),
            bundle_version: "2026.07.3".to_string(),
            bundle_revision: "artifact-runtime-bundle-sha256-v1:test".to_string(),
            kind: AgentCommandRuntimeKind::Python,
            runtime_version: "3.12.13".to_string(),
            runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
            resolved_packages: vec![AgentCommandRuntimeResolvedPackage {
                name: "pypdf".to_string(),
                version: "5.0.0".to_string(),
            }],
        })),
        managed_office_script: None,
    };

    let renderer_event = agent_event_notification(AgentEvent::Done {
        run_id: "run-pdf".to_string(),
        success: false,
        status: Some(AgentRunStatus::WaitingForApproval),
        content: None,
        usage: None,
        finish_reason: None,
        proposed_actions: vec![AgentProposedAction::Command { command: request }],
    });
    let rendered = serde_json::to_string(&renderer_event).unwrap();
    assert!(!rendered.contains(private_artifact_path));
    assert!(!rendered.contains(artifact_uri));
    assert!(!rendered.contains("runtimeFingerprint"));
    assert!(!rendered.contains("runtimeBinding"));
    assert!(renderer_event["params"]["proposedActions"][0]["command"]
        .get("inputs")
        .is_none());
    assert_eq!(
        renderer_event["params"]["proposedActions"][0]["command"]["command"],
        "pdfinfo '$MYCOPILOT_INPUT_ROOT/manual.pdf'"
    );
}

#[test]
fn renderer_command_projection_excludes_managed_office_script_authority() {
    let mut projection = serde_json::json!({
        "type": "command",
        "command": {
            "id": "command-office-editor",
            "command": "node edit_deck.mjs --output edited.pptx",
            "cwd": null,
            "timeoutMs": 120000,
            "approvalStatus": "required",
            "riskLevel": null,
            "reason": "edit the presentation",
            "observe": null,
            "inputs": [],
            "runtimeBinding": { "runtimeFingerprint": "private-runtime" },
            "managedOfficeScript": {
                "destination": "/private/office/staging/edited.pptx",
                "planPath": "/private/office/staging/edit-plan.json"
            }
        }
    });

    redact_renderer_mcp_binding_fields(&mut projection);

    let command = projection["command"].as_object().unwrap();
    assert!(!command.contains_key("inputs"));
    assert!(!command.contains_key("runtimeBinding"));
    assert!(!command.contains_key("managedOfficeScript"));
    assert_eq!(
        command.get("command").and_then(Value::as_str),
        Some("node edit_deck.mjs --output edited.pptx")
    );
    assert!(!serde_json::to_string(&projection)
        .unwrap()
        .contains("/private/office/staging"));
}

#[test]
fn renderer_file_change_projection_excludes_all_execution_authority() {
    const CANARY: &str = "PRIVATE_FILE_CHANGE_EXECUTION_CANARY";
    let file_change_action = serde_json::json!({
        "type": "file_change",
        "fileChange": {
            "schemaVersion": 1,
            "id": "call-commit",
            "transactionId": "transaction-1",
            "operation": "update",
            "updateStrategy": "rewrite",
            "filePath": "report.md",
            "inlineDiff": null,
            "baseRevision": null,
            "summary": null,
            "additions": 1,
            "deletions": 0,
            "lineCount": 1,
            "byteCount": 8,
            "approvalStatus": "required",
            "execution": {
                "canonicalTarget": CANARY,
                "baseContent": CANARY,
                "targetContent": CANARY,
                "permissionRevision": CANARY,
                "toolSetRevision": CANARY,
                "providerWireRevision": CANARY,
                "privateObservation": CANARY
            }
        }
    });
    let projections = [
        ("proposed action", file_change_action.clone()),
        (
            "standalone proposal event",
            serde_json::json!({
                "type": "file_change_proposed",
                "runId": "run-1",
                "fileChange": file_change_action["fileChange"].clone()
            }),
        ),
        (
            "nested observer event",
            serde_json::json!({
                "schemaVersion": 1,
                "event": {
                    "type": "file_change_proposed",
                    "runId": "run-1",
                    "fileChange": file_change_action["fileChange"].clone()
                }
            }),
        ),
    ];

    for (boundary, mut projection) in projections {
        redact_renderer_mcp_binding_fields(&mut projection);

        let serialized = serde_json::to_string(&projection).unwrap();
        assert!(
            !serialized.contains("\"execution\"") && !serialized.contains(CANARY),
            "{boundary} leaked Host-private FileChange authority"
        );
        assert!(
            serialized.contains("transaction-1"),
            "{boundary} dropped the public FileChange identity"
        );
    }
}

#[test]
fn pending_continuation_uses_original_model_args_not_backend_bound_builder_fields() {
    let original_args = serde_json::json!({
        "command": "python scripts/builder.py --output report.docx",
        "reason": "build the document"
    });
    let resolved_packages = vec![AgentCommandRuntimeResolvedPackage {
        name: "python-docx".to_string(),
        version: "1.2.0".to_string(),
    }];
    let profile_revision_material = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        "profile": AgentCommandRuntimeProfile::Documents,
        "kind": AgentCommandRuntimeKind::Python,
        "packages": resolved_packages,
    }))
    .unwrap();
    let profile_revision = format!(
        "artifact-runtime-profile-sha256-v1:{:x}",
        Sha256::digest(profile_revision_material)
    );
    let command = AgentCommandRequest {
        id: "command-backend-bound".to_string(),
        command: "python scripts/builder.py --output report.docx".to_string(),
        cwd: None,
        timeout_ms: Some(120_000),
        approval_status: mycopilot_core::AgentApprovalStatus::Required,
        risk_level: None,
        reason: Some("build the document".to_string()),
        observe: Some(AgentCommandArtifactObservationRequest {
            kinds: vec![AgentCommandArtifactObservationKind::Office],
            expected_outputs: vec!["report.docx".to_string()],
            additional_roots: Vec::new(),
        }),
        inputs: Vec::new(),
        runtime_binding: Some(Box::new(AgentCommandRuntimeBinding {
            schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
            profile: AgentCommandRuntimeProfile::Documents,
            profile_revision,
            provider_id: "mycopilot.artifact-runtime".to_string(),
            bundle_version: "2026.07.3".to_string(),
            bundle_revision: "artifact-runtime-bundle-sha256-v1:test".to_string(),
            kind: AgentCommandRuntimeKind::Python,
            runtime_version: "3.12.13".to_string(),
            runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
            resolved_packages,
        })),
        managed_office_script: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-backend-bound".to_string(),
        pending_action_id: None,
        file_change_run_grant_ref: None,
        pending_file_observation: None,
        context_items: vec![AgentContextCheckpointItem {
            role: "assistant".to_string(),
            content: String::new(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: command.id.clone(),
                name: "run_command".to_string(),
                args: original_args.clone(),
                provider_identity: mycopilot_core::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: command.id.clone(),
                    runtime_call_id: command.id.clone(),
                },
            }],
            is_error: false,
            sources: vec!["tool_call".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            group: None,
            origin: None,
        }],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[command.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: command.id.clone(),
        conversation_model_context_items: Vec::new(),
        conversation_trace_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    });
    let mut record = PendingActionRecord {
        storage_id: "pending-backend-bound".to_string(),
        snapshot: PendingAgentActionSnapshot {
            action_id: command.id.clone(),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            tool_call_id: Some(command.id.clone()),
            run_id: "run-backend-bound".to_string(),
            conversation_id: None,
            assistant_message_id: None,
            action: AgentProposedAction::Command { command },
            created_at: 1,
            status: PendingActionStatus::Pending,
        },
        agent_input,
    };

    let continuation = tool_call_for_pending_record(&record).unwrap();
    assert_eq!(continuation.args, original_args);
    assert!(continuation.args.get("runtimeProfile").is_none());
    assert!(continuation.args.get("observe").is_none());

    record
        .agent_input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .context_items[0]
        .tool_calls[0]
        .args["command"] = serde_json::json!("python scripts/other.py --output report.docx");
    let error = tool_call_for_pending_record(&record).unwrap_err();
    assert!(error.contains("run_command 参数与冻结 action 不一致"));

    let checkpoint = record.agent_input.resume_checkpoint.as_mut().unwrap();
    checkpoint.context_items[0].tool_calls[0].args = original_args;
    let duplicate = checkpoint.context_items[0].tool_calls[0].clone();
    checkpoint.context_items[0].tool_calls.push(duplicate);
    let error = tool_call_for_pending_record(&record).unwrap_err();
    assert!(error.contains("重复的原始模型 ToolCall"));
}

#[test]
fn failed_command_tool_result_keeps_the_complete_execution_observation() {
    let execution = AgentCommandExecutionResult {
        outputs: Vec::new(),
        output_capture: Default::default(),
        stdout_spool: Default::default(),
        stderr_spool: Default::default(),
        command: "python3 -c 'import openpyxl'".to_string(),
        cwd: ".".to_string(),
        exit_code: Some(1),
        stdout: "dependency check started".to_string(),
        stderr: "ModuleNotFoundError: No module named 'openpyxl'".to_string(),
        timed_out: false,
        cancelled: false,
        duration_ms: 25,
        stdout_truncated: false,
        stderr_truncated: true,
        error: None,
        policy_evaluation: None,
        artifact_observation: None,
        input_files: Vec::new(),
        runtime: None,
        managed_outputs: None,
        authoritative_archive_ref: None,
        history_open: None,
    };

    let result = command_tool_result("command-1", &execution);
    let observation = result.result.expect("structured command observation");

    assert!(!result.ok);
    assert_eq!(result.error.as_deref(), Some("命令执行失败。"));
    assert_eq!(observation["exitCode"], 1);
    assert_eq!(observation["stdout"], "dependency check started");
    assert_eq!(
        observation["stderr"],
        "ModuleNotFoundError: No module named 'openpyxl'"
    );
    assert_eq!(observation["timedOut"], false);
    assert_eq!(observation["cancelled"], false);
    assert_eq!(observation["stdoutTruncated"], false);
    assert_eq!(observation["stderrTruncated"], true);
}

#[test]
fn policy_rejection_keeps_stable_structured_diagnostics_in_tool_result() {
    use mycopilot_core::command::{
        evaluate_command_policy, CommandAuthorizationSource, CommandPolicyDecision,
    };
    use mycopilot_core::AgentCommandSafetyPolicy;

    let request = AgentCommandRequest {
        id: "command-policy-1".to_string(),
        command: "rm -rf /".to_string(),
        cwd: None,
        timeout_ms: Some(5_000),
        approval_status: mycopilot_core::AgentApprovalStatus::Approved,
        risk_level: None,
        reason: None,
        observe: None,
        inputs: Vec::new(),
        runtime_binding: None,
        managed_office_script: None,
    };
    let evaluation = evaluate_command_policy(
        &request.command,
        AgentCommandSafetyPolicy::FullAccess,
        CommandAuthorizationSource::Automatic,
    );
    assert_eq!(evaluation.decision, CommandPolicyDecision::Deny);
    let execution = failed_command_result(
        &request,
        "命令已被安全策略拒绝。".to_string(),
        Some(evaluation),
    );

    let result = command_tool_result(&request.id, &execution);
    let observation = result.result.expect("structured policy observation");

    assert_eq!(observation["policyEvaluation"]["decision"], "deny");
    assert_eq!(
        observation["policyEvaluation"]["code"],
        "command.catastrophic.filesystem_root"
    );
    assert_eq!(
        observation["policyEvaluation"]["findings"][0]["risk"],
        "catastrophic"
    );
}
