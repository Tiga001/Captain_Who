use super::*;
use crate::completed_conversation_trace_without_items;
use crate::image_generation::{
    ImageArtifactFormat, ImageGenerationExecutionFailureCode, ImageGenerationExecutionPhase,
};
use serde_json::{json, Value};

#[test]
fn model_request_interruption_marker_survives_error_enrichment() {
    let error = AgentError::structured(
        "agent.llm_provider_failure",
        "模型服务网络请求失败。",
        json!({
            "type": "llm_provider_failure",
            "category": "network",
        }),
    )
    .with_model_request_interruption()
    .with_usage(Some(AgentUsage {
        input_tokens: Some(7),
        output_tokens: None,
        output_thinking_tokens: None,
        total_tokens: Some(7),
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        billable_request_count: Some(1),
    }))
    .with_conversation_turn_trace(completed_conversation_trace_without_items(
        "run-1",
        "conversation-1",
        "assistant-1",
    ));

    assert_eq!(
        error.model_request_interruption(),
        Some(AgentModelRequestInterruptionReason::ServiceConnectionFailed)
    );
    assert_eq!(error.usage().and_then(|usage| usage.total_tokens), Some(7));
    assert!(error.conversation_turn_trace().is_some());
}

#[test]
fn provider_continuation_ref_is_versioned_canonical_and_debug_redacted() {
    let continuation_ref = ProviderContinuationRef::new();
    continuation_ref.validate().unwrap();
    assert!(continuation_ref.id.starts_with("provider-continuation-v1:"));
    assert_eq!(
        format!("{continuation_ref:?}"),
        "ProviderContinuationRef([REDACTED])"
    );
    assert!(!format!("{continuation_ref:?}").contains(&continuation_ref.id));

    let encoded = serde_json::to_value(&continuation_ref).unwrap();
    assert_eq!(encoded["version"], PROVIDER_CONTINUATION_REF_VERSION);
    assert_eq!(encoded["id"], continuation_ref.id);
    assert_eq!(
        serde_json::from_value::<ProviderContinuationRef>(encoded).unwrap(),
        continuation_ref
    );
}

#[test]
fn provider_continuation_ref_rejects_unknown_or_noncanonical_identity() {
    let valid = ProviderContinuationRef::new();
    assert!(ProviderContinuationRef::parse(valid.version + 1, valid.id.clone()).is_err());
    assert!(ProviderContinuationRef::parse(
        PROVIDER_CONTINUATION_REF_VERSION,
        "provider-continuation-v1:00000000-0000-0000-0000-000000000000"
    )
    .is_err());
    assert!(ProviderContinuationRef::parse(
        PROVIDER_CONTINUATION_REF_VERSION,
        valid.id.to_ascii_uppercase()
    )
    .is_err());
}

fn image_generation_audit() -> AgentImageGenerationAudit {
    AgentImageGenerationAudit {
        execution_id: format!("agent-v1:{}", "a".repeat(64)),
        request_fingerprint: format!("sha256:{}", "b".repeat(64)),
        provider_profile_id: "default".to_string(),
        adapter_id: "smartmlSeedream".to_string(),
        profile_revision: 7,
        model_id: "seedream-model".to_string(),
        provider_request_id: Some(format!("sha256:{}", "c".repeat(32))),
        http_status: Some(200),
        created_at: 10,
        completed_at: 20,
        duration_ms: 10,
    }
}

#[test]
fn agent_events_match_the_cross_language_golden_contract() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../packages/protocol/fixtures/agent-contract-v1.json"
    ))
    .unwrap();
    let events = &fixture["events"];

    let message_delta = AgentEvent::MessageDelta {
        run_id: "run-contract-v1".to_string(),
        stream_id: Some("stream-contract-v1".to_string()),
        delta: "hello".to_string(),
    };
    let message_stream_committed = AgentEvent::MessageStreamCommitted {
        run_id: "run-contract-v1".to_string(),
        stream_id: "stream-contract-v1".to_string(),
        trace_sequence: Some(3),
    };
    let tool_call = AgentEvent::ToolCall {
        run_id: "run-contract-v1".to_string(),
        trace_sequence: 4,
        call: AgentToolCall {
            id: "call-contract-v1".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": "README.md" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: Some("Inspect project documentation.".to_string()),
        },
    };
    let tool_result = AgentEvent::ToolResult {
        run_id: "run-contract-v1".to_string(),
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: "call-contract-v1".to_string(),
            tool: "read_file".to_string(),
            ok: true,
            result: Some(json!({ "content": "MyCopilot Next" })),
            error: None,
        },
    };
    let command_started = AgentEvent::CommandStarted {
        run_id: "run-contract-v1".to_string(),
        conversation_id: "conversation-contract-v1".to_string(),
        assistant_message_id: "assistant-contract-v1".to_string(),
        project_id: Some("project-contract-v1".to_string()),
        call_id: "call-command-contract-v1".to_string(),
        session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
        started_at: 10,
    };
    let command_output = AgentEvent::CommandOutput {
        run_id: "run-contract-v1".to_string(),
        conversation_id: "conversation-contract-v1".to_string(),
        assistant_message_id: "assistant-contract-v1".to_string(),
        project_id: Some("project-contract-v1".to_string()),
        call_id: "call-command-contract-v1".to_string(),
        session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
        sequence: 1,
        stream: AgentCommandOutputStream::Stdout,
        output: "ready\n".to_string(),
    };
    let command_exited = AgentEvent::CommandExited {
        run_id: "run-contract-v1".to_string(),
        conversation_id: "conversation-contract-v1".to_string(),
        assistant_message_id: "assistant-contract-v1".to_string(),
        project_id: Some("project-contract-v1".to_string()),
        call_id: "call-command-contract-v1".to_string(),
        session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
        status: AgentCommandExitStatus::Exited,
        exit_code: Some(0),
        ended_at: 20,
        latest_sequence: 1,
        output_truncated: false,
        outputs: Vec::new(),
        artifact_observation: None,
    };
    let command_interrupted = AgentEvent::CommandInterrupted {
        run_id: "run-contract-v1".to_string(),
        conversation_id: "conversation-contract-v1".to_string(),
        assistant_message_id: "assistant-contract-v1".to_string(),
        project_id: Some("project-contract-v1".to_string()),
        call_id: "call-command-contract-v1".to_string(),
        session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
        ended_at: 21,
        latest_sequence: 1,
        output_truncated: false,
        outputs: Vec::new(),
        artifact_observation: None,
    };
    let done = AgentEvent::Done {
        run_id: "run-contract-v1".to_string(),
        success: true,
        status: Some(AgentRunStatus::Completed),
        content: Some("done".to_string()),
        usage: None,
        finish_reason: None,
        proposed_actions: Vec::new(),
    };

    assert_eq!(
        serde_json::to_value(message_delta).unwrap(),
        events["messageDelta"]
    );
    assert_eq!(
        serde_json::to_value(message_stream_committed).unwrap(),
        events["messageStreamCommitted"]
    );
    assert_eq!(serde_json::to_value(tool_call).unwrap(), events["toolCall"]);
    assert_eq!(
        serde_json::to_value(tool_result).unwrap(),
        events["toolResult"]
    );
    assert_eq!(
        serde_json::to_value(command_started).unwrap(),
        events["commandStarted"]
    );
    assert_eq!(
        serde_json::to_value(command_output).unwrap(),
        events["commandOutput"]
    );
    assert_eq!(
        serde_json::to_value(command_exited).unwrap(),
        events["commandExited"]
    );
    assert_eq!(
        serde_json::to_value(command_interrupted).unwrap(),
        events["commandInterrupted"]
    );
    assert_eq!(serde_json::to_value(done).unwrap(), events["done"]);
}

#[test]
fn managed_command_session_projection_is_camel_case_and_process_safe() {
    let snapshot = AgentCommandSessionSnapshot {
        schema_version: 2,
        session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
        conversation_id: "conversation-1".to_string(),
        assistant_message_id: "assistant-1".to_string(),
        origin_run_id: "run-1".to_string(),
        call_id: "call-1".to_string(),
        project_id: Some("project-1".to_string()),
        command: "python3 app.py".to_string(),
        cwd: "/workspace".to_string(),
        command_digest: format!("sha256:{}", "a".repeat(64)),
        status: AgentCommandSessionStatus::Running,
        started_at: 10,
        ended_at: None,
        exit_code: None,
        latest_sequence: 2,
        output_truncated: false,
        outputs: Vec::new(),
        artifact_observation: None,
        archive_ref: None,
    };
    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(value["schemaVersion"], 2);
    assert_eq!(value["sessionId"], snapshot.session_id);
    assert_eq!(value["originRunId"], "run-1");
    assert_eq!(value["status"], "running");
    for forbidden in ["pid", "environment", "approvalPayload", "processHandle"] {
        assert!(value.get(forbidden).is_none(), "leaked {forbidden}");
    }

    let started = serde_json::to_value(AgentEvent::CommandStarted {
        run_id: "run-1".to_string(),
        conversation_id: "conversation-1".to_string(),
        assistant_message_id: "assistant-1".to_string(),
        project_id: Some("project-1".to_string()),
        call_id: "call-1".to_string(),
        session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
        started_at: 10,
    })
    .unwrap();
    assert_eq!(started["type"], "command_started");
    assert_eq!(started["conversationId"], "conversation-1");
    assert_eq!(started["assistantMessageId"], "assistant-1");
    assert_eq!(started["projectId"], "project-1");
    assert_eq!(started["sessionId"], snapshot.session_id);
}

#[test]
fn image_generation_success_contract_exposes_only_managed_artifact_metadata() {
    let digest = "d".repeat(64);
    let result = AgentImageGenerationResult {
        schema_version: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
        status: AgentImageGenerationResultStatus::Succeeded,
        operation: AgentImageGenerationOperation::Generate,
        artifact: Some(AgentImageGenerationArtifact {
            artifact_id: format!("sha256:{digest}"),
            uri: format!("image-artifact://sha256/{digest}"),
            kind: AgentImageGenerationArtifactKind::Image,
            format: ImageArtifactFormat::Png,
            mime_type: "image/png".to_string(),
            width: 1024,
            height: 1024,
            size_bytes: 42,
            sha256: digest.clone(),
        }),
        audit: image_generation_audit(),
        failure: None,
    };

    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["status"], "succeeded");
    assert_eq!(value["operation"], "generate");
    assert_eq!(value["artifact"]["kind"], "image");
    assert_eq!(value["artifact"]["format"], "png");
    assert_eq!(
        value["artifact"]["uri"],
        format!("image-artifact://sha256/{digest}")
    );
    assert_eq!(value["artifact"].as_object().unwrap().len(), 9);
    assert!(value.get("failure").is_none());

    let serialized = serde_json::to_string(&result).unwrap();
    for forbidden in [
        "endpoint",
        "signedUrl",
        "storageRelativePath",
        "absolutePath",
        "apiKey",
        "inputBytes",
        "https://provider.example/signed-output",
        "PRIVATE_API_KEY",
    ] {
        assert!(!serialized.contains(forbidden), "leaked {forbidden}");
    }

    let round_trip: AgentImageGenerationResult = serde_json::from_value(value).unwrap();
    assert_eq!(round_trip, result);
}

#[test]
fn image_generation_failure_contract_preserves_uncertainty_without_artifact() {
    let result = AgentImageGenerationResult {
        schema_version: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
        status: AgentImageGenerationResultStatus::OutcomeIndeterminate,
        operation: AgentImageGenerationOperation::Edit,
        artifact: None,
        audit: image_generation_audit(),
        failure: Some(AgentImageGenerationFailure {
            code: ImageGenerationExecutionFailureCode::DeadlineExceeded,
            phase: ImageGenerationExecutionPhase::Provider,
            message: "the provider outcome is unknown".to_string(),
            recovery: "Check execution history before retrying.".to_string(),
            retryable: false,
            generation_may_have_succeeded: true,
            provider_succeeded: false,
            artifact_commit_may_have_succeeded: false,
        }),
    };

    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["status"], "outcomeIndeterminate");
    assert_eq!(value["operation"], "edit");
    assert!(value.get("artifact").is_none());
    assert_eq!(value["failure"]["code"], "deadlineExceeded");
    assert_eq!(value["failure"]["phase"], "provider");
    assert_eq!(value["failure"]["generationMayHaveSucceeded"], true);
    assert_eq!(value["failure"]["providerSucceeded"], false);
    assert_eq!(value["failure"]["artifactCommitMayHaveSucceeded"], false);
}

#[test]
fn image_generation_terminal_statuses_have_stable_wire_values() {
    let cases = [
        (AgentImageGenerationResultStatus::Succeeded, "succeeded"),
        (AgentImageGenerationResultStatus::Failed, "failed"),
        (AgentImageGenerationResultStatus::Cancelled, "cancelled"),
        (
            AgentImageGenerationResultStatus::OutcomeIndeterminate,
            "outcomeIndeterminate",
        ),
        (
            AgentImageGenerationResultStatus::CommitIndeterminate,
            "commitIndeterminate",
        ),
    ];
    for (status, expected) in cases {
        assert_eq!(serde_json::to_value(status).unwrap(), expected);
    }
}

#[test]
fn skill_activated_event_uses_the_stable_frontend_contract() {
    let value = serde_json::to_value(AgentEvent::SkillActivated {
        run_id: "run-1".to_string(),
        activation_revision: "activation-sha256-v1:test".to_string(),
        activated_by: AgentSkillActivationActor::Model,
        skill: AgentSkillActivatedEvent {
            id: "bundled:application:documents".to_string(),
            name: "documents".to_string(),
            revision: "skill-package-sha256-v2:test".to_string(),
            source: AgentSkillSourceSummary {
                kind: "bundled".to_string(),
                id: "bundled:application".to_string(),
            },
        },
    })
    .unwrap();

    assert_eq!(value["type"], "skill_activated");
    assert_eq!(value["runId"], "run-1");
    assert_eq!(value["activationRevision"], "activation-sha256-v1:test");
    assert_eq!(value["activatedBy"], "model");
    assert_eq!(value["skill"]["name"], "documents");
    assert_eq!(value["skill"]["source"]["kind"], "bundled");
    assert_eq!(value["skill"]["source"]["id"], "bundled:application");
}

#[test]
fn model_capabilities_are_required_and_use_current_camel_case_shape() {
    let missing = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "text-only-model",
        "messages": []
    }));
    assert!(missing.is_err());

    let input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "text-only-model",
        "modelCapabilities": { "imageInput": true },
        "messages": []
    }))
    .unwrap();
    let serialized = serde_json::to_value(&input).unwrap();
    assert_eq!(serialized["modelCapabilities"]["imageInput"], true);
    assert!(serialized.get("model_capabilities").is_none());

    let round_trip = serde_json::from_value::<AgentChatInput>(serialized).unwrap();
    assert!(round_trip.model_capabilities.image_input);
}

#[test]
fn agent_input_calls_and_results_never_expose_values_through_debug() {
    const CANARY: &str = "AGENT_RUNTIME_DEBUG_SECRET_CANARY";
    let input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": CANARY,
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "searchConfig": {
            "mode": "tavily",
            "tavilyApiKey": CANARY
        },
        "messages": []
    }))
    .unwrap();
    let call = AgentToolCall {
        id: "call-safe-id".to_string(),
        tool: "mcp__fixture__echo".to_string(),
        args: json!({"neutral": CANARY}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let result = AgentToolResult {
        call_id: "call-safe-id".to_string(),
        tool: "mcp__fixture__echo".to_string(),
        ok: true,
        result: Some(json!({"neutral": CANARY})),
        error: None,
        exact_archive_file: None,
    };

    let rendered = format!("{input:?}{:?}{call:?}{result:?}", input.search_config);
    assert!(!rendered.contains(CANARY));
    assert!(!rendered.contains("neutral"));
}

#[test]
fn current_permissions_require_explicit_command_safety_policy() {
    let missing = serde_json::from_value::<AgentPermissions>(json!({
        "read": "all",
        "write": "all",
        "command": "auto_approve",
        "patch": "auto_approve"
    }));
    assert!(missing.is_err());

    let permissions: AgentPermissions = serde_json::from_value(json!({
        "read": "all",
        "write": "all",
        "command": "auto_approve",
        "commandSafety": "guarded",
        "patch": "auto_approve"
    }))
    .unwrap();
    assert_eq!(
        permissions.command_safety,
        AgentCommandSafetyPolicy::Guarded
    );
}

#[test]
fn command_artifact_observation_v3_is_total_and_rejects_unknown_fields() {
    let current = AgentCommandArtifactObservation {
        schema_version: AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
        status: AgentCommandArtifactObservationStatus::Complete,
        partial: false,
        stop_reasons: Vec::new(),
        scanned: 1,
        returned: 0,
        omitted: 0,
        coverage: AgentCommandArtifactObservationCoverage {
            workspace_included: true,
            expected_output_count: 0,
            additional_root_count: 0,
            before: AgentCommandArtifactSnapshotCoverage::default(),
            after: AgentCommandArtifactSnapshotCoverage::default(),
        },
        changes: Vec::new(),
        changes_truncated: false,
        changes_omitted: 0,
        expected_outputs: Vec::new(),
        warnings: Vec::new(),
    };
    let canonical = serde_json::to_value(&current).unwrap();
    let decoded: AgentCommandArtifactObservation =
        serde_json::from_value(canonical.clone()).unwrap();
    assert_eq!(decoded, current);

    for field in [
        "partial",
        "stopReasons",
        "scanned",
        "returned",
        "omitted",
        "changesTruncated",
        "changesOmitted",
        "expectedOutputs",
        "warnings",
    ] {
        let mut missing = canonical.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<AgentCommandArtifactObservation>(missing).is_err(),
            "current artifact observation field {field} is required"
        );
    }

    let mut extra = canonical;
    extra
        .as_object_mut()
        .unwrap()
        .insert("executionAuthority".to_string(), json!(true));
    assert!(serde_json::from_value::<AgentCommandArtifactObservation>(extra).is_err());
}

#[test]
fn persisted_action_nested_settings_reject_unknown_fields() {
    let mut observation = json!({
        "kinds": ["office"],
        "expectedOutputs": [],
        "additionalRoots": []
    });
    observation["executionAuthority"] = true.into();
    assert!(serde_json::from_value::<AgentCommandArtifactObservationRequest>(observation).is_err());

    let mut preflight = json!({
        "status": "ready",
        "interpreter": "python3",
        "interpreterVersion": "3.13",
        "dependencies": [{
            "kind": "command",
            "name": "python3",
            "status": "available",
            "version": "3.13"
        }],
        "runtimeFingerprint": "sha256:current"
    });
    preflight["dependencies"][0]["runtimeBinding"] = json!({});
    assert!(serde_json::from_value::<AgentSkillScriptPreflightReport>(preflight).is_err());

    let installation = json!({
        "schemaVersion": AGENT_SKILL_INSTALLATION_SCHEMA_VERSION,
        "id": "install-1",
        "installRef": "private-ref",
        "preview": {
            "name": "example",
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
            "warnings": [{
                "code": "review",
                "message": "Review source",
                "requiresAcknowledgement": true
            }],
            "compatibility": "compatible",
            "operation": "install",
            "impact": "Adds one Skill"
        },
        "approvalStatus": "required",
        "expiresAt": 100
    });
    serde_json::from_value::<AgentSkillInstallationRequest>(installation.clone()).unwrap();
    for path in ["preview", "resourceSummary", "warning"] {
        let mut extra = installation.clone();
        match path {
            "preview" => extra["preview"]["runtimeBinding"] = json!({}),
            "resourceSummary" => extra["preview"]["resourceSummary"]["runtimeBinding"] = json!({}),
            "warning" => extra["preview"]["warnings"][0]["runtimeBinding"] = json!({}),
            _ => unreachable!(),
        }
        assert!(
            serde_json::from_value::<AgentSkillInstallationRequest>(extra).is_err(),
            "nested {path} must reject extra fields"
        );
    }
}

#[test]
fn current_skill_activation_requires_total_skill_snapshots() {
    let activation = AgentSkillActivation {
        activation_revision: "activation-sha256-v1:current".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:documents".to_string(),
            name: "documents".to_string(),
            revision: "skill-sha256-v1:current".to_string(),
            source: "bundled".to_string(),
            instructions: "Use the current instructions.".to_string(),
            source_bytes: 29,
            resources: None,
        }],
    };
    let canonical = serde_json::to_value(&activation).unwrap();
    serde_json::from_value::<AgentSkillActivation>(canonical.clone()).unwrap();

    let mut missing_skills = canonical.clone();
    missing_skills.as_object_mut().unwrap().remove("skills");
    assert!(serde_json::from_value::<AgentSkillActivation>(missing_skills).is_err());

    let mut missing_source_bytes = canonical.clone();
    missing_source_bytes["skills"][0]
        .as_object_mut()
        .unwrap()
        .remove("sourceBytes");
    assert!(serde_json::from_value::<AgentSkillActivation>(missing_source_bytes).is_err());

    let mut extra = canonical;
    extra["skills"][0]
        .as_object_mut()
        .unwrap()
        .insert("runtimeBinding".to_string(), json!({}));
    assert!(serde_json::from_value::<AgentSkillActivation>(extra).is_err());
}

#[test]
fn command_safety_policy_uses_stable_camel_case_field_and_wire_value() {
    let permissions = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::All,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        patch: AgentPatchPermission::AutoApprove,
    };

    let serialized = serde_json::to_value(permissions).unwrap();

    assert_eq!(serialized["commandSafety"], "full_access");
    assert!(serialized.get("command_safety").is_none());
}

#[test]
fn chat_message_serializes_conversation_trace_with_camel_case_protocol_names() {
    let message = AgentChatMessage {
        message_id: None,
        role: "assistant".to_string(),
        content: "done".to_string(),
        created_at: Some(0),
        conversation_turn_trace: Some(completed_conversation_trace_without_items(
            "run-1",
            "conversation-1",
            "assistant-1",
        )),
        conversation_model_context_items: Vec::new(),
    };
    let serialized = serde_json::to_string(&message).unwrap();

    assert!(serialized.contains("\"conversationTurnTrace\""));
    assert!(serialized.contains("\"createdAt\":0"));
    assert!(!serialized.contains("conversation_turn_trace"));
}

#[test]
fn skill_activation_uses_camel_case_and_redacts_instructions_from_debug() {
    let activation = AgentSkillActivation {
        activation_revision: "activation-sha256-v1:test".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "workspace:w:review".to_string(),
            name: "review".to_string(),
            revision: "skill-sha256-v1:test".to_string(),
            source: "workspace".to_string(),
            instructions: "PRIVATE_SKILL_INSTRUCTIONS".to_string(),
            source_bytes: 26,
            resources: None,
        }],
    };

    let serialized = serde_json::to_value(&activation).unwrap();
    assert_eq!(
        serialized["activationRevision"],
        "activation-sha256-v1:test"
    );
    assert_eq!(
        serialized["skills"][0]["instructions"],
        "PRIVATE_SKILL_INSTRUCTIONS"
    );
    assert!(serialized.get("activation_revision").is_none());
    assert!(!format!("{activation:?}").contains("PRIVATE_SKILL_INSTRUCTIONS"));
}

#[test]
fn context_compaction_events_serialize_stable_identity_and_outcome() {
    let started = serde_json::to_value(AgentEvent::ContextCompactionStarted {
        run_id: "run-1".to_string(),
        operation_id: "compaction-1".to_string(),
        trace_sequence: 7,
    })
    .unwrap();
    let finished = serde_json::to_value(AgentEvent::ContextCompactionFinished {
        run_id: "run-1".to_string(),
        operation_id: "compaction-1".to_string(),
        outcome: AgentContextCompactionEventOutcome::Applied,
        trace_sequence: 7,
    })
    .unwrap();

    assert_eq!(started["type"], "context_compaction_started");
    assert_eq!(started["operationId"], "compaction-1");
    assert_eq!(started["traceSequence"], 7);
    assert_eq!(finished["type"], "context_compaction_finished");
    assert_eq!(finished["operationId"], started["operationId"]);
    assert_eq!(finished["outcome"], "applied");
    assert_eq!(finished["traceSequence"], started["traceSequence"]);
}

#[test]
fn sequenced_presentation_events_serialize_required_trace_fields() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../packages/protocol/fixtures/agent-contract-v1.json"
    ))
    .unwrap();
    assert_eq!(fixture["events"]["toolCall"]["traceSequence"], 4);
    assert_eq!(
        fixture["events"]["messageStreamCommitted"]["traceSequence"],
        3
    );

    let stream_without_narration = serde_json::to_value(AgentEvent::MessageStreamCommitted {
        run_id: "run-1".to_string(),
        stream_id: "stream-1".to_string(),
        trace_sequence: None,
    })
    .unwrap();
    let unanchored_error = serde_json::to_value(AgentEvent::Error {
        run_id: Some("run-1".to_string()),
        trace_sequence: None,
        message: "failed".to_string(),
        recoverable: false,
        code: None,
        details: None,
    })
    .unwrap();
    assert!(stream_without_narration
        .as_object()
        .unwrap()
        .contains_key("traceSequence"));
    assert!(stream_without_narration["traceSequence"].is_null());
    assert!(unanchored_error
        .as_object()
        .unwrap()
        .contains_key("traceSequence"));
    assert!(unanchored_error["traceSequence"].is_null());
}

#[test]
fn llm_retry_event_serializes_structured_safe_retry_metadata() {
    let retry = serde_json::to_value(AgentEvent::LlmRetry {
        run_id: "run-1".to_string(),
        stream_id: "stream-1".to_string(),
        attempt: 2,
        max_attempts: 6,
        category: "rate_limited".to_string(),
        provider_code: Some("rate_limit_exceeded".to_string()),
        delay_ms: 5_000,
        retry_at: 1_800_000_005_000,
    })
    .unwrap();

    assert_eq!(retry["type"], "llm_retry");
    assert_eq!(retry["category"], "rate_limited");
    assert_eq!(retry["providerCode"], "rate_limit_exceeded");
    assert_eq!(retry["delayMs"], 5_000);
    assert_eq!(retry["retryAt"], 1_800_000_005_000_u64);
    assert_eq!(retry["attempt"], 2);
    assert_eq!(retry["maxAttempts"], 6);
    assert!(retry.get("reason").is_none());
}

#[test]
fn agent_permission_meet_is_component_wise_and_never_widens() {
    let full = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::All,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        patch: AgentPatchPermission::AutoApprove,
    };
    let custom = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::RequireApproval,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        patch: AgentPatchPermission::AutoApprove,
    };
    let minimum = AgentPermissions::default();

    assert_eq!(full.meet(custom), custom);
    assert_eq!(custom.meet(full), custom);
    assert_eq!(custom.meet(minimum), minimum);
    assert_eq!(minimum.meet(custom), minimum);
    assert_eq!(full.meet(full), full);
    assert_eq!(minimum.meet(minimum), minimum);
}
