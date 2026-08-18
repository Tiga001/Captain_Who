use super::*;

#[test]
fn approval_checkpoint_rejects_private_queued_tool_arguments_without_leaking_them() {
    let secret = "fixture-token-that-must-not-persist";
    let call_id = canonical_test_call_id(1, "provider-private-mcp");
    let provider_call = LlmToolCall {
        id: "provider-private-mcp".to_string(),
        name: "mcp__fixture__credential_tool".to_string(),
        args: json!({"token": secret, "query": "safe"}),
    };
    let queued = QueuedToolCall {
        provider_call,
        provider_tool_index: 1,
        call: LlmToolCall {
            id: call_id.clone(),
            name: "mcp__fixture__credential_tool".to_string(),
            args: json!({"token": secret, "query": "safe"}),
        },
        checkpoint_call: LlmToolCall {
            id: call_id,
            name: "mcp__fixture__credential_tool".to_string(),
            args: json!({"token": "[redacted MCP argument]", "query": "safe"}),
        },
        checkpoint_persistence: AgentToolCallCheckpointPersistence::DeniedMcp,
        assistant_content: String::new(),
        group_id: "run:checkpoint-validation-run:tool-exchange:1:2".to_string(),
    };

    let error = queued_tool_call_checkpoint(&queued, "assistant-turn-test", false).unwrap_err();
    assert_eq!(
        error.code(),
        Some("agent.checkpoint_private_tool_arguments")
    );
    assert!(!error.to_string().contains(secret));
    assert!(!error
        .details()
        .is_some_and(|details| details.to_string().contains(secret)));
}

#[test]
fn approval_checkpoint_rejects_all_mcp_calls_even_when_projection_cannot_detect_secret() {
    let secret = "Bearer fixture-neutral-field-secret";
    let call_id = canonical_test_call_id(1, "provider-neutral-mcp");
    let call = LlmToolCall {
        id: call_id,
        name: "provider_visible_name_without_routing_semantics".to_string(),
        args: json!({"text": secret}),
    };
    let queued = QueuedToolCall {
        provider_call: LlmToolCall {
            id: "provider-neutral-mcp".to_string(),
            name: call.name.clone(),
            args: call.args.clone(),
        },
        provider_tool_index: 1,
        checkpoint_call: call.clone(),
        call,
        checkpoint_persistence: AgentToolCallCheckpointPersistence::DeniedMcp,
        assistant_content: String::new(),
        group_id: "run:checkpoint-validation-run:tool-exchange:1:2".to_string(),
    };

    let error = queued_tool_call_checkpoint(&queued, "assistant-turn-test", false).unwrap_err();
    assert_eq!(
        error.code(),
        Some("agent.checkpoint_private_tool_arguments")
    );
    assert!(!error.to_string().contains(secret));
    assert!(!error
        .details()
        .is_some_and(|details| details.to_string().contains(secret)));
}

#[test]
fn deepseek_checkpoint_rehydrates_private_mcp_queue_from_authenticated_turn() {
    let secret = "deepseek-encrypted-mcp-secret";
    let pending = LlmToolCall {
        id: canonical_test_call_id(0, "provider-mcp-pending"),
        name: "mcp__fixture__first".to_string(),
        args: json!({"value": "first"}),
    };
    let queued = LlmToolCall {
        id: canonical_test_call_id(1, "provider-mcp-queued"),
        name: "mcp__fixture__second".to_string(),
        args: json!({"token": secret}),
    };
    let mut batch = ToolCallBatch::from_model_response(
        "checkpoint-validation-run",
        0,
        String::new(),
        vec![pending.clone(), queued.clone()],
        false,
        |call| {
            (
                LlmToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: json!({}),
                },
                AgentToolCallCheckpointPersistence::DeniedMcp,
            )
        },
    );
    let authenticated_turn = batch.take_assistant_turn().unwrap();
    let authenticated_group =
        ContextGroup::tool_exchange(format!("provider-turn:{}", authenticated_turn.stable_id()));
    pop_test_call(&mut batch, &pending.id);

    let safe_checkpoint = queued_tool_call_checkpoint(
        batch.queue.front().unwrap(),
        &authenticated_turn.stable_id(),
        true,
    )
    .unwrap();
    let checkpoint_json = serde_json::to_string(&safe_checkpoint).unwrap();
    assert!(!checkpoint_json.contains(secret));

    let placeholder = batch.queue.front_mut().unwrap();
    placeholder.call.args = json!({});
    placeholder.provider_call.args = json!({});
    batch
        .rehydrate_queued_calls_from_provider_turn(&authenticated_turn, |call| {
            (
                LlmToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: json!({}),
                },
                AgentToolCallCheckpointPersistence::DeniedMcp,
            )
        })
        .unwrap();
    assert_eq!(batch.queue.front().unwrap().call.args["token"], secret);
    assert_eq!(batch.queue.front().unwrap().checkpoint_call.args, json!({}));
    assert_eq!(
        batch.queue.front().unwrap().checkpoint_persistence,
        AgentToolCallCheckpointPersistence::DeniedMcp
    );
    assert_eq!(
        batch.queue.front().unwrap().context_group(),
        authenticated_group
    );
}

#[test]
fn mcp_approval_barrier_drops_only_external_calls_and_persists_a_safe_reprepare_count() {
    let secret = "queued-mcp-secret-must-not-persist";
    let pending = LlmToolCall {
        id: canonical_test_call_id(0, "provider-pending"),
        name: "write_file".to_string(),
        args: json!({"path": "report.txt"}),
    };
    let mut batch = ToolCallBatch::from_model_response(
        "checkpoint-validation-run",
        0,
        String::new(),
        vec![
            pending.clone(),
            LlmToolCall {
                id: canonical_test_call_id(1, "provider-mcp-one"),
                name: "mcp__fixture__first".to_string(),
                args: json!({"value": secret}),
            },
            LlmToolCall {
                id: canonical_test_call_id(2, "provider-builtin"),
                name: "read_file".to_string(),
                args: json!({"path": "report.txt"}),
            },
            LlmToolCall {
                id: canonical_test_call_id(3, "provider-mcp-two"),
                name: "mcp__fixture__second".to_string(),
                args: json!({"other": secret}),
            },
        ],
        false,
        |call| {
            (
                LlmToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: if call.name.starts_with("mcp__") {
                        json!({})
                    } else {
                        call.args.clone()
                    },
                },
                if call.name.starts_with("mcp__") {
                    AgentToolCallCheckpointPersistence::DeniedMcp
                } else {
                    AgentToolCallCheckpointPersistence::Allowed
                },
            )
        },
    );
    let checkpoint_message = batch.checkpoint_assistant_message().unwrap().unwrap();
    let group = batch.context_group().unwrap();
    let complete_turn = batch.take_assistant_turn().unwrap();
    pop_test_call(&mut batch, &pending.id);
    let mut context = ContextFrame::new(vec![ContextItem::new(
        LlmMessage::from_assistant_turn(complete_turn),
        ContextMetadata::new(
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_group(group.clone()),
    )
    .with_checkpoint_message(checkpoint_message)]);

    let deferred = batch.defer_external_calls(|queued| queued.call.name.starts_with("mcp__"));
    assert_eq!(deferred.len(), 2);
    let deferred_ids = deferred
        .into_iter()
        .map(|queued| queued.call.id)
        .collect::<BTreeSet<_>>();
    context
        .omit_runtime_tool_calls_from_group(&group, &deferred_ids)
        .unwrap();
    assert_eq!(batch.queue.len(), 1);
    assert_eq!(batch.queue[0].call.name, "read_file");
    assert_eq!(batch.take_deferred_external_tool_call_count(), None);

    let trace = current_test_pending_trace(&batch, &pending);
    let checkpoint = create_run_checkpoint(
        "checkpoint-validation-run",
        RunCheckpointState {
            context: &context,
            next_model_request_index: 1,
            tool_batch: &batch,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &pending.id,
            conversation_trace: &trace,
            tool_set: &test_tool_set(),
            run_context: None,
            collaboration_run_snapshot: None,
            model_capabilities: ModelCapabilities::default(),
            run_world_state: &test_run_world_state(),
            provider_profile_config: &test_provider_profile(),
            provider_protocol_key: &test_provider_key(),
        },
    )
    .unwrap();

    assert_eq!(checkpoint.queued_tool_calls.len(), 1);
    assert_eq!(checkpoint.deferred_external_tool_call_count, 2);
    let rendered = serde_json::to_string(&checkpoint).unwrap();
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("mcp__fixture__first"));
    assert!(!rendered.contains("mcp__fixture__second"));

    batch.pop_front();
    assert_eq!(batch.take_deferred_external_tool_call_count(), Some(2));
    assert_eq!(batch.take_deferred_external_tool_call_count(), None);
}

#[test]
fn current_checkpoint_schema_round_trips_and_rejects_missing_or_extra_fields() {
    let (checkpoint, _) = restorable_checkpoint_fixture();
    let canonical = serde_json::to_value(&checkpoint).unwrap();
    let decoded: AgentRunCheckpoint = serde_json::from_value(canonical.clone()).unwrap();
    assert_eq!(decoded, checkpoint);

    for field in [
        "deferredExternalToolCallCount",
        "providerContinuationRefs",
        "runContext",
        "collaborationRunSnapshot",
        "conversationTraceItems",
        "conversationModelContextItems",
        "nextConversationTraceSequence",
        "conversationTraceTruncated",
    ] {
        let mut missing = canonical.clone();
        missing.as_object_mut().unwrap().remove(field);
        let error = serde_json::from_value::<AgentRunCheckpoint>(missing).unwrap_err();
        assert!(
            error.to_string().contains(field),
            "missing {field} must be rejected explicitly: {error}"
        );
    }

    assert!(canonical["runContext"].is_null());
    assert!(canonical["collaborationRunSnapshot"].is_null());

    let mut missing_provider_identity = canonical.clone();
    missing_provider_identity["contextItems"][0]["toolCalls"][0]
        .as_object_mut()
        .expect("current Tool Call checkpoint")
        .remove("providerIdentity");
    assert!(
        serde_json::from_value::<AgentRunCheckpoint>(missing_provider_identity).is_err(),
        "persisted Tool Calls must carry their exact Provider/Runtime identity"
    );

    for path in [
        &["modelCapabilities", "imageInput"][..],
        &["providerProtocolKey", "providerConfigurationRevision"][..],
    ] {
        let mut missing = canonical.clone();
        missing[path[0]]
            .as_object_mut()
            .expect("current nested checkpoint object")
            .remove(path[1]);
        assert!(
            serde_json::from_value::<AgentRunCheckpoint>(missing).is_err(),
            "missing nested checkpoint field {}.{} must fail closed",
            path[0],
            path[1]
        );
    }

    let mut extra_capability = canonical.clone();
    extra_capability["modelCapabilities"]
        .as_object_mut()
        .unwrap()
        .insert("providerPolicy".to_string(), json!(true));
    assert!(serde_json::from_value::<AgentRunCheckpoint>(extra_capability).is_err());

    let mut missing_batch_capabilities = canonical.clone();
    missing_batch_capabilities["toolSet"]
        .as_object_mut()
        .unwrap()
        .remove("activeCapabilityIds");
    assert!(
        serde_json::from_value::<AgentRunCheckpoint>(missing_batch_capabilities).is_err(),
        "v9 must freeze the request-boundary capability set separately from post-effect extension state"
    );

    let mut extra_world_state = canonical.clone();
    extra_world_state["runWorldState"]
        .as_object_mut()
        .unwrap()
        .insert("legacyEpoch".to_string(), json!(true));
    assert!(serde_json::from_value::<AgentRunCheckpoint>(extra_world_state).is_err());

    let mut extra_world_section = canonical.clone();
    extra_world_section["runWorldState"]["sections"][0]
        .as_object_mut()
        .unwrap()
        .insert("runtimeCapability".to_string(), json!(true));
    assert!(serde_json::from_value::<AgentRunCheckpoint>(extra_world_section).is_err());

    let mut checkpoint_with_context = checkpoint.clone();
    checkpoint_with_context.run_context = Some(crate::protocol::AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: Some(crate::protocol::AgentWorkspaceContext {
            project_id: None,
            display_name: Some("Current workspace".to_string()),
            root_path: Some("/current/workspace".to_string()),
        }),
        attachment_library: Some(crate::protocol::AgentAttachmentLibraryContext {
            root_path: None,
            conversation_id: None,
            project_id: None,
            conversation_attachments: Vec::new(),
            project_attachments: Vec::new(),
        }),
        permissions: crate::protocol::AgentPermissions::default(),
    });
    let context_json = serde_json::to_value(checkpoint_with_context).unwrap();
    for field in ["conversationId", "projectId", "workspace", "permissions"] {
        let mut missing = context_json.clone();
        missing["runContext"].as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<AgentRunCheckpoint>(missing).is_err(),
            "current runContext field {field} must be explicit"
        );
    }
    for field in ["projectId", "displayName", "rootPath"] {
        let mut missing = context_json.clone();
        missing["runContext"]["workspace"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            serde_json::from_value::<AgentRunCheckpoint>(missing).is_err(),
            "current workspace field {field} must be explicit"
        );
    }
    for field in ["conversationAttachments", "projectAttachments"] {
        let mut missing = context_json.clone();
        missing["runContext"]["attachmentLibrary"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            serde_json::from_value::<AgentRunCheckpoint>(missing).is_err(),
            "current attachment library field {field} must be explicit"
        );
    }
    let mut extra_context = context_json;
    extra_context["runContext"]
        .as_object_mut()
        .unwrap()
        .insert("continuationPolicy".to_string(), json!("forged"));
    assert!(serde_json::from_value::<AgentRunCheckpoint>(extra_context).is_err());

    let mut legacy_skill_barrier = canonical.clone();
    legacy_skill_barrier["queuedToolCalls"][0]
        .as_object_mut()
        .unwrap()
        .insert("deferredBySkillActivation".to_string(), json!(true));
    assert!(
        serde_json::from_value::<AgentRunCheckpoint>(legacy_skill_barrier).is_err(),
        "the v9 queued Tool Call shape must reject the retired activation barrier"
    );

    let mut extra = canonical;
    extra
        .as_object_mut()
        .unwrap()
        .insert("legacyCheckpointField".to_string(), json!(true));
    assert!(
        serde_json::from_value::<AgentRunCheckpoint>(extra).is_err(),
        "unknown checkpoint fields must fail closed"
    );
}

#[test]
fn approval_checkpoint_freezes_selector_and_wait_admission_across_resume() {
    let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
    let frozen_directory = crate::AgentCollaborationSelectorDirectory::bounded(
        Vec::new(),
        vec![crate::AgentCollaborationModelSelector {
            model_config_id: "model-visible-before-approval".to_string(),
            display_name: "Visible before approval".to_string(),
            capabilities: ModelCapabilities { image_input: true },
        }],
    );
    let frozen_snapshot = crate::AgentCollaborationRunSnapshot {
        selector_directory: frozen_directory.clone(),
        admitted_wait_model_batches: vec![3],
    };
    checkpoint.tool_set = collaboration_tool_set().checkpoint();
    checkpoint.collaboration_run_snapshot = Some(frozen_snapshot.clone());
    checkpoint.model_capabilities = ModelCapabilities { image_input: true };
    checkpoint.run_world_state = test_run_world_state_for(true);

    let checkpoint_json = serde_json::to_value(&checkpoint).unwrap();
    assert_eq!(
        checkpoint_json["collaborationRunSnapshot"]["selectorDirectory"]["models"][0]
            ["capabilities"]["imageInput"],
        true
    );
    let mut missing_capability = checkpoint_json;
    missing_capability["collaborationRunSnapshot"]["selectorDirectory"]["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("capabilities");
    assert!(serde_json::from_value::<AgentRunCheckpoint>(missing_capability).is_err());

    let serialized = serde_json::to_vec(&checkpoint).unwrap();
    let checkpoint: AgentRunCheckpoint = serde_json::from_slice(&serialized).unwrap();
    let restored =
        restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
    assert_eq!(
        restored.collaboration_run_snapshot.as_ref(),
        Some(&frozen_snapshot)
    );
    assert_eq!(
        restored.model_capabilities,
        ModelCapabilities { image_input: true }
    );

    let current_services = crate::AgentCollaborationRuntimeServices::new(
        std::sync::Arc::new(NeverCollaborationExecutor),
        test_collaboration_caller(),
        crate::AgentCollaborationSelectorDirectory::bounded(
            Vec::new(),
            vec![crate::AgentCollaborationModelSelector {
                model_config_id: "model-added-during-approval".to_string(),
                display_name: "Added during approval".to_string(),
                capabilities: ModelCapabilities { image_input: false },
            }],
        ),
    )
    .with_run_snapshot(
        restored
            .collaboration_run_snapshot
            .as_ref()
            .expect("checkpoint has frozen collaboration authority"),
    )
    .unwrap();
    let authorization = current_services.selector_authorization();
    assert!(authorization.allows_model_config_id("model-visible-before-approval"));
    assert!(!authorization.allows_model_config_id("model-added-during-approval"));
    assert_eq!(
        authorization.expected_model_capabilities(None, Some("model-visible-before-approval")),
        Some(ModelCapabilities { image_input: true })
    );
    assert!(!current_services.try_admit_wait_model_batch(3).unwrap());
    assert!(current_services.try_admit_wait_model_batch(4).unwrap());
}
