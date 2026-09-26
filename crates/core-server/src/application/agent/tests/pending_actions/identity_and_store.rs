use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::mcp_fixtures::{test_mcp_call_id, test_mcp_pending_action, test_mcp_resume_checkpoint};
use super::*;
use mycopilot_core::{
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimeProfile,
    AgentCommandRuntimeResolvedPackage, AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

#[test]
fn pending_command_round_trip_keeps_the_host_frozen_runtime_binding() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "secret",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": true },
        "messages": []
    }))
    .unwrap();
    let resolved_packages = vec![AgentCommandRuntimeResolvedPackage {
        name: "pptxgenjs".to_string(),
        version: "4.0.1".to_string(),
    }];
    let profile_revision_material = serde_json::to_vec(&json!({
        "schemaVersion": AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        "profile": AgentCommandRuntimeProfile::Presentations,
        "kind": AgentCommandRuntimeKind::Node,
        "packages": resolved_packages,
    }))
    .unwrap();
    let profile_revision = format!(
        "artifact-runtime-profile-sha256-v1:{:x}",
        Sha256::digest(profile_revision_material)
    );
    let binding = AgentCommandRuntimeBinding {
        schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        profile: AgentCommandRuntimeProfile::Presentations,
        profile_revision,
        provider_id: mycopilot_core::artifact_runtime::ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
        bundle_version: mycopilot_core::artifact_runtime::ARTIFACT_RUNTIME_BUNDLE_VERSION
            .to_string(),
        bundle_revision: "artifact-runtime-bundle-sha256-v1:test".to_string(),
        kind: AgentCommandRuntimeKind::Node,
        runtime_version: "22.23.1".to_string(),
        runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
        resolved_packages,
    };
    let action = AgentProposedAction::Command {
        command: AgentCommandRequest {
            id: "managed-runtime-profile-pending".to_string(),
            command: "node scripts/build.mjs --output deck.pptx".to_string(),
            cwd: None,
            timeout_ms: Some(30_000),
            approval_status: AgentApprovalStatus::Required,
            risk_level: None,
            reason: Some("build an Office artifact".to_string()),
            observe: Some(mycopilot_core::AgentCommandArtifactObservationRequest {
                kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
                expected_outputs: vec!["deck.pptx".to_string()],
                additional_roots: Vec::new(),
            }),
            inputs: Vec::new(),
            runtime_binding: Some(Box::new(binding.clone())),
            managed_office_script: None,
        },
    };

    let model_call = AgentToolCall {
        id: "managed-runtime-profile-pending".to_string(),
        tool: "run_command".to_string(),
        args: json!({ "command": "node scripts/build.mjs --output deck.pptx" }),
        approval_status: AgentApprovalStatus::Required,
        reason: Some("build an Office artifact".to_string()),
    };
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "managed-runtime-profile-run",
        None,
        &model_call,
        AgentToolIdentity::Builtin {
            tool_name: "run_command".to_string(),
        },
    ));
    let AgentProposedAction::Command { command } = &action else {
        unreachable!();
    };
    mycopilot_core::validate_frozen_agent_command_args(command, &model_call.args).unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    assert!(pending_action_binding_matches(
        "managed-runtime-profile-run",
        None,
        &action,
        &agent_input,
    ));
    assert!(service
        .store_pending_action(
            "managed-runtime-profile-run",
            "managed-runtime-profile-conversation",
            "managed-runtime-profile-assistant",
            action,
            agent_input,
        )
        .unwrap());
    drop(service);

    let reloaded = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let pending = reloaded
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let stored = pending
        .get(&pending_action_storage_id(
            "managed-runtime-profile-run",
            "managed-runtime-profile-pending",
        ))
        .unwrap();
    assert!(stored.agent_input.model_capabilities.image_input);
    let AgentProposedAction::Command { command } = &stored.snapshot.action else {
        panic!("persisted action must remain a command")
    };
    assert_eq!(command.runtime_binding.as_deref(), Some(&binding));
    let resumed_call = tool_call_for_pending_record(stored).unwrap();
    assert!(resumed_call.args.get("runtimeProfile").is_none());
    assert!(resumed_call.args["runtime"].is_null());
}

#[test]
fn provider_action_id_is_scoped_by_run_and_same_run_reuse_is_strict() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    let original = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "shared-action-id".to_string(),
            tool: "first_tool".to_string(),
            args: json!({ "value": "original" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let AgentProposedAction::ToolCall {
        call: original_call,
    } = &original
    else {
        unreachable!();
    };
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-original",
        None,
        original_call,
        AgentToolIdentity::Unregistered {
            tool_name: original_call.tool.clone(),
        },
    ));
    assert!(service
        .store_pending_action(
            "run-original",
            "conversation-original",
            "assistant-original",
            original.clone(),
            agent_input.clone(),
        )
        .unwrap());
    assert!(!service
        .store_pending_action(
            "run-original",
            "conversation-original",
            "assistant-original",
            original,
            agent_input.clone(),
        )
        .unwrap());

    let cross_run = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "shared-action-id".to_string(),
            tool: "second_tool".to_string(),
            args: json!({ "value": "replacement" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    let AgentProposedAction::ToolCall {
        call: cross_run_call,
    } = &cross_run
    else {
        unreachable!();
    };
    let mut cross_run_input = agent_input.clone();
    cross_run_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-second",
        None,
        cross_run_call,
        AgentToolIdentity::Unregistered {
            tool_name: cross_run_call.tool.clone(),
        },
    ));
    assert!(service
        .store_pending_action(
            "run-second",
            "conversation-second",
            "assistant-second",
            cross_run,
            cross_run_input,
        )
        .unwrap());
    let same_run_collision = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "shared-action-id".to_string(),
            tool: "conflicting_tool".to_string(),
            args: json!({ "value": "conflict" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    let AgentProposedAction::ToolCall {
        call: collision_call,
    } = &same_run_collision
    else {
        unreachable!();
    };
    let mut collision_input = agent_input;
    collision_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-original",
        None,
        collision_call,
        AgentToolIdentity::Unregistered {
            tool_name: collision_call.tool.clone(),
        },
    ));
    let error = service
        .store_pending_action(
            "run-original",
            "conversation-original",
            "assistant-original",
            same_run_collision,
            collision_input,
        )
        .unwrap_err();
    assert!(error.contains("冻结快照冲突"));

    let in_memory = service
        .pending_actions
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner());
    let frozen = in_memory
        .get(&pending_action_storage_id(
            "run-original",
            "shared-action-id",
        ))
        .unwrap();
    assert_eq!(frozen.snapshot.run_id, "run-original");
    assert_eq!(frozen.snapshot.tool_name, "first_tool");
    drop(in_memory);
    let durable = storage.list_pending_agent_actions().unwrap();
    assert_eq!(durable.len(), 2);
    assert!(durable.iter().any(|record| {
        record.run_id == "run-original" && record.action_json.contains("first_tool")
    }));
    assert!(durable.iter().any(|record| {
        record.run_id == "run-second" && record.action_json.contains("second_tool")
    }));
    assert!(durable
        .iter()
        .all(|record| !record.action_json.contains("conflicting_tool")));
}

#[test]
fn mcp_pending_and_checkpoint_json_freeze_only_the_safe_payload_capability() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let run_id = "mcp-safe-persistence-marker";
    let action_id = uuid::Uuid::new_v4().to_string();
    let action = test_mcp_pending_action(
        run_id,
        &action_id,
        &uuid::Uuid::new_v4().to_string(),
        mycopilot_core::storage::now_ms(),
    );
    let checkpoint = test_mcp_resume_checkpoint(&storage, run_id, &action_id);
    let persisted = serde_json::to_string(&json!({
        "pendingAction": action,
        "checkpoint": checkpoint,
    }))
    .unwrap();

    assert!(persisted.contains("\"payloadPersistence\":\"process_only\""));
    for forbidden in [
        "payloadRef",
        "opaquePayloadRef",
        "ciphertext",
        "credentialRef",
        "secretRef",
    ] {
        assert!(
            !persisted.contains(forbidden),
            "safe pending/checkpoint JSON must not contain {forbidden}"
        );
    }
}

#[test]
fn pending_store_rejects_missing_checkpoint_or_exact_context_before_persistence() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let mut base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut base_input);
    let now = mycopilot_core::storage::now_ms();

    let missing_action_id = uuid::Uuid::new_v4().to_string();
    let error = service
        .store_pending_action(
            "missing-checkpoint-run",
            "missing-checkpoint-conversation",
            "missing-checkpoint-assistant",
            test_mcp_pending_action(
                "missing-checkpoint-run",
                &missing_action_id,
                &uuid::Uuid::new_v4().to_string(),
                now,
            ),
            base_input.clone(),
        )
        .unwrap_err();
    assert_eq!(
        error,
        "Pending action frozen Tool Call identity is inconsistent."
    );

    let empty_context_action_id = uuid::Uuid::new_v4().to_string();
    let mut empty_context_input = base_input;
    let mut checkpoint =
        test_mcp_resume_checkpoint(&storage, "empty-context-run", &empty_context_action_id);
    checkpoint.context_items.clear();
    empty_context_input.resume_checkpoint = Some(checkpoint);
    let error = service
        .store_pending_action(
            "empty-context-run",
            "empty-context-conversation",
            "empty-context-assistant",
            test_mcp_pending_action(
                "empty-context-run",
                &empty_context_action_id,
                &uuid::Uuid::new_v4().to_string(),
                now,
            ),
            empty_context_input,
        )
        .unwrap_err();
    assert_eq!(
        error,
        "Pending action frozen Tool Call identity is inconsistent."
    );

    assert!(storage.list_pending_agent_actions().unwrap().is_empty());
    assert!(service
        .pending_actions
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner())
        .is_empty());
}

#[test]
fn mcp_pending_identity_mismatch_is_rejected_on_store_and_malformed_resume_is_retired() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let mut base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut base_input);
    let now = mycopilot_core::storage::now_ms();

    let rejected_run_id = "mcp-identity-rejected-run";
    let rejected_action_id = uuid::Uuid::new_v4().to_string();
    let rejected_invocation_id = uuid::Uuid::new_v4().to_string();
    let mut rejected_input = base_input.clone();
    rejected_input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &storage,
        rejected_run_id,
        &uuid::Uuid::new_v4().to_string(),
    ));
    let error = service
        .store_pending_action(
            rejected_run_id,
            "mcp-identity-rejected-conversation",
            "mcp-identity-rejected-assistant",
            test_mcp_pending_action(
                rejected_run_id,
                &rejected_action_id,
                &rejected_invocation_id,
                now,
            ),
            rejected_input,
        )
        .unwrap_err();
    assert_eq!(
        error,
        "Pending action frozen Tool Call identity is inconsistent."
    );
    assert!(storage.list_pending_agent_actions().unwrap().is_empty());

    let persisted_run_id = "mcp-identity-persisted-run";
    let persisted_conversation_id = "mcp-identity-persisted-conversation";
    let persisted_assistant_message_id = "mcp-identity-persisted-assistant";
    let persisted_action_id = uuid::Uuid::new_v4().to_string();
    let persisted_invocation_id = uuid::Uuid::new_v4().to_string();
    let persisted_action = test_mcp_pending_action(
        persisted_run_id,
        &persisted_action_id,
        &persisted_invocation_id,
        now,
    );
    let AgentProposedAction::McpToolCall { approval } = &persisted_action else {
        unreachable!();
    };
    seed_durable_pending_owner(
        &storage,
        persisted_conversation_id,
        persisted_assistant_message_id,
        persisted_run_id,
        &approval.call,
        AgentToolIdentity::Mcp {
            provenance: approval.identity.provenance.clone(),
        },
        now,
    );
    let mut persisted_input = base_input;
    persisted_input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &storage,
        persisted_run_id,
        &persisted_action_id,
    ));
    assert!(service
        .store_pending_action(
            persisted_run_id,
            persisted_conversation_id,
            persisted_assistant_message_id,
            persisted_action,
            persisted_input,
        )
        .unwrap());
    let storage_id = pending_action_storage_id(persisted_run_id, &persisted_action_id);
    let row = storage
        .list_pending_agent_actions()
        .unwrap()
        .into_iter()
        .find(|row| row.action_id == storage_id)
        .unwrap();
    let mut tampered_input = serde_json::from_str::<serde_json::Value>(&row.agent_input_json)
        .expect("versioned projection must be valid JSON");
    tampered_input["resumeCheckpoint"]["pendingActionId"] =
        serde_json::Value::String(uuid::Uuid::new_v4().to_string());
    storage
        .transition_pending_agent_action(
            &storage_id,
            "pending",
            "pending",
            &tampered_input.to_string(),
            now + 1,
        )
        .unwrap();
    drop(service);

    let restarted = AgentService::try_new(Arc::clone(&storage))
        .expect("malformed private resume state must retire from durable ToolCall identity");
    assert!(restarted
        .pending_actions
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner())
        .is_empty());
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let retired: (String, String, String) = connection
        .query_row(
            "SELECT status, action_json, agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        retired,
        ("failed".to_string(), "{}".to_string(), "{}".to_string())
    );
}

#[test]
fn malformed_non_mcp_pending_states_retire_without_restoring_an_executable_action() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let now = mycopilot_core::storage::now_ms();
    let mut storage_ids = Vec::new();

    for status in ["pending", "approved", "executing"] {
        let run_id = format!("malformed-{status}-run");
        let conversation_id = format!("malformed-{status}-conversation");
        let assistant_message_id = format!("malformed-{status}-assistant");
        let call = AgentToolCall {
            id: format!("malformed-{status}-call"),
            tool: "approval_tool".to_string(),
            args: json!({}),
            approval_status: if status == "pending" {
                AgentApprovalStatus::Required
            } else {
                AgentApprovalStatus::Approved
            },
            reason: None,
        };
        seed_durable_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call,
            AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
            now,
        );
        let storage_id = pending_action_storage_id(&run_id, &call.id);
        storage
            .store_pending_agent_action(AgentPendingActionRecord {
                action_id: storage_id.clone(),
                run_id,
                conversation_id: Some(conversation_id),
                assistant_message_id: Some(assistant_message_id),
                action_type: "tool_call".to_string(),
                tool_name: call.tool.clone(),
                tool_call_id: Some(call.id),
                status: status.to_string(),
                target_status: None,
                action_json: json!({ "unsupportedAction": true }).to_string(),
                agent_input_json: json!({ "unsupportedResume": true }).to_string(),
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        storage_ids.push(storage_id);
    }

    let loaded = load_persisted_pending_actions(&storage)
        .expect("malformed private projections must retire from durable ToolCall identity");
    assert!(loaded.is_empty());
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());

    let connection = rusqlite::Connection::open(database_path).unwrap();
    for storage_id in storage_ids {
        let retired: (String, String, String) = connection
            .query_row(
                "SELECT status, action_json, agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            retired,
            ("failed".to_string(), "{}".to_string(), "{}".to_string())
        );
    }
}

#[test]
fn mcp_pending_binding_checks_checkpoint_run_storage_call_and_tool_identity() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let run_id = "mcp-binding-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let action = test_mcp_pending_action(
        run_id,
        &action_id,
        &invocation_id,
        mycopilot_core::storage::now_ms(),
    );
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, run_id, &action_id));
    let model_tool_name = match &action {
        AgentProposedAction::McpToolCall { approval } => {
            approval.identity.provenance.model_tool_name.clone()
        }
        _ => unreachable!(),
    };
    let mut record = AgentPendingActionRecord {
        action_id: pending_action_storage_id(run_id, &action_id),
        run_id: run_id.to_string(),
        conversation_id: None,
        assistant_message_id: None,
        action_type: "mcp_tool_call".to_string(),
        tool_name: model_tool_name,
        tool_call_id: Some(test_mcp_call_id(&action_id)),
        status: "pending".to_string(),
        target_status: None,
        action_json: "{}".to_string(),
        agent_input_json: "{}".to_string(),
        created_at: 1,
        updated_at: 1,
    };
    let AgentProposedAction::McpToolCall { approval } = &action else {
        unreachable!();
    };
    let event_validation = mcp_tool_invocation_event(
        approval,
        McpToolInvocationEventUpdate {
            state: AgentMcpToolInvocationState::PendingApproval,
            dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            outcome: None,
            is_error: None,
            error_code: None,
            duration_ms: None,
            output_truncated: false,
            result_size: None,
            failure_stage: None,
        },
    );
    assert!(
        event_validation.is_ok(),
        "fixture approval must satisfy Core validation: {event_validation:?}"
    );
    assert!(pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));

    record.tool_call_id = Some("tampered-call".to_string());
    assert!(!pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));
    record.tool_call_id = Some(test_mcp_call_id(&action_id));
    record.action_id = "tampered-storage-id".to_string();
    assert!(!pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));
    record.action_id = pending_action_storage_id(run_id, &action_id);
    record.tool_name = "tampered-tool".to_string();
    assert!(!pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));
    record.tool_name = match &action {
        AgentProposedAction::McpToolCall { approval } => {
            approval.identity.provenance.model_tool_name.clone()
        }
        _ => unreachable!(),
    };
    record.run_id = "tampered-run".to_string();
    assert!(!pending_action_binding_matches(
        &record.run_id,
        Some(&record),
        &action,
        &input
    ));
    record.run_id = run_id.to_string();
    input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .pending_tool_call_id = "tampered-call".to_string();
    assert!(!pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));
}
