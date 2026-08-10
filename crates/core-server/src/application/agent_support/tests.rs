use super::*;
use mycopilot_core::{
    AgentCommandArtifactObservationKind, AgentCommandArtifactObservationRequest,
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimePackageRequirement,
    AgentCommandRuntimeProfile, AgentCommandRuntimeProvider, AgentCommandRuntimeRequest,
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
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call_id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_action_id: Some(action_id),
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
fn command_tool_call_projects_only_the_model_visible_runtime_profile() {
    let request = AgentCommandRequest {
        id: "command-runtime-profile-1".to_string(),
        command: "node scripts/build.mjs".to_string(),
        cwd: Some("workspace".to_string()),
        timeout_ms: Some(30_000),
        approval_status: mycopilot_core::AgentApprovalStatus::Required,
        risk_level: None,
        reason: Some("build a presentation".to_string()),
        observe: None,
        inputs: Vec::new(),
        runtime: None,
        runtime_binding: Some(Box::new(AgentCommandRuntimeBinding {
            schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
            profile: AgentCommandRuntimeProfile::Presentations,
            profile_revision: "artifact-runtime-profile-sha256-v1:test".to_string(),
            provider_id: "mycopilot.artifact-runtime".to_string(),
            bundle_version: "2026.07.3".to_string(),
            bundle_revision: "artifact-runtime-bundle-sha256-v1:test".to_string(),
            kind: AgentCommandRuntimeKind::Node,
            runtime_version: "22.23.1".to_string(),
            runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
            resolved_packages: vec![AgentCommandRuntimeResolvedPackage {
                name: "pptxgenjs".to_string(),
                version: "4.0.1".to_string(),
            }],
        })),
    };

    let call = command_tool_call(&request);
    assert_eq!(call.args["runtimeProfile"], "presentations");
    assert!(call.args["runtime"].is_null());
    assert!(call.args.get("timeoutMs").is_none());
    assert_eq!(request.timeout_ms, Some(30_000));
    let model_args = serde_json::to_string(&call.args).unwrap();
    assert!(!model_args.contains("pptxgenjs"));
    assert!(!model_args.contains("4.0.1"));
    assert!(!model_args.contains("runtimeFingerprint"));
}

#[test]
fn command_tool_call_keeps_the_frozen_managed_runtime_request() {
    let request = AgentCommandRequest {
        id: "command-runtime-1".to_string(),
        command: "node scripts/build.mjs".to_string(),
        cwd: Some("workspace".to_string()),
        timeout_ms: Some(30_000),
        approval_status: mycopilot_core::AgentApprovalStatus::Required,
        risk_level: None,
        reason: Some("build a workbook".to_string()),
        observe: None,
        inputs: Vec::new(),
        runtime: Some(AgentCommandRuntimeRequest {
            provider: AgentCommandRuntimeProvider::ManagedArtifact,
            kind: AgentCommandRuntimeKind::Node,
            required_packages: vec![AgentCommandRuntimePackageRequirement {
                name: "exceljs".to_string(),
                version: "4.4.0".to_string(),
            }],
        }),
        runtime_binding: None,
    };

    let call = command_tool_call(&request);

    assert_eq!(call.args["runtime"]["provider"], "managedArtifact");
    assert_eq!(call.args["runtime"]["kind"], "node");
    assert_eq!(
        call.args["runtime"]["requiredPackages"][0]["name"],
        "exceljs"
    );
    assert_eq!(
        call.args["runtime"]["requiredPackages"][0]["version"],
        "4.4.0"
    );
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
        runtime: None,
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
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-backend-bound".to_string(),
        pending_action_id: None,
        context_items: vec![AgentContextCheckpointItem {
            role: "assistant".to_string(),
            content: String::new(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: command.id.clone(),
                name: "run_command".to_string(),
                args: original_args.clone(),
                provider_identity: None,
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
        runtime: None,
        runtime_binding: None,
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
