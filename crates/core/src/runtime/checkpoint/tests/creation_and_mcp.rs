use super::*;

fn builtin_activation_identity() -> AgentToolIdentity {
    AgentToolIdentity::RuntimeExtension {
        extension_id: crate::builtin_capabilities::BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID
            .to_string(),
        tool_name: crate::builtin_capabilities::ACTIVATE_CAPABILITY_TOOL_NAME.to_string(),
    }
}

fn freeze_pending_provenance(checkpoint: &mut AgentRunCheckpoint, provenance: AgentToolIdentity) {
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let frozen = checkpoint
        .conversation_trace_items
        .iter_mut()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id,
                provenance,
                ..
            } if call_id == &pending_call_id => Some(provenance),
            _ => None,
        })
        .expect("pending ToolCall has frozen provenance");
    *frozen = provenance;
}

fn freeze_pending_approval_status(
    checkpoint: &mut AgentRunCheckpoint,
    approval_status: AgentApprovalStatus,
) {
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let frozen = checkpoint
        .conversation_trace_items
        .iter_mut()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id,
                approval_status,
                ..
            } if call_id == &pending_call_id => Some(approval_status),
            _ => None,
        })
        .expect("pending ToolCall has frozen approval status");
    *frozen = approval_status;
}

fn builtin_sensitive_identity(tool_name: &str) -> AgentToolIdentity {
    AgentToolIdentity::BuiltinCapability {
        capability_id: "browser_automation".into(),
        managed_mcp_id: "builtin.browser_automation.mcp".into(),
        package_name: "@playwright/mcp".into(),
        package_version: "0.0.79".into(),
        upstream_catalog_digest: format!("sha256:{}", "a".repeat(64)).into(),
        policy_digest: format!("sha256:{}", "b".repeat(64)).into(),
        manifest_digest: format!("sha256:{}", "c".repeat(64)).into(),
        tool_id: tool_name.to_string().into(),
        raw_name: tool_name.to_string().into(),
        model_name: tool_name.to_string().into(),
        upstream_schema_digest: format!("sha256:{}", "d".repeat(64)).into(),
        host_overlay_digest: format!("sha256:{}", "e".repeat(64)).into(),
        host_input_schema_digest: format!("sha256:{}", "f".repeat(64)).into(),
    }
}

fn observer_setup_trace(
    checkpoint: &AgentRunCheckpoint,
    continuation: &AgentToolContinuation,
    assistant_message_id: &str,
) -> crate::ConversationTraceSnapshot {
    let mut input: crate::AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "checkpoint-test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(checkpoint.clone());
    input.tool_continuation = Some(continuation.clone());
    input.assistant_message_id = Some(assistant_message_id.to_string());
    crate::runtime::conversation_trace_from_input_checkpoint(
        &input,
        &ModelToolResultGate::new(crate::context::ContextTextBudget::heuristic(
            crate::context::MODEL_TOOL_RESULT_MAX_TOKENS,
        )),
        &ConversationHistoryArchiveTraceMetadata::default(),
    )
    .unwrap()
    .snapshot()
}

#[test]
fn builtin_activation_action_id_preserves_the_standard_tool_result_prefix() {
    let (mut checkpoint, mut continuation) =
        restorable_checkpoint_fixture_for_pending_tool("activate_capability");
    checkpoint.pending_action_id = Some(uuid::Uuid::new_v4().to_string());
    freeze_pending_provenance(&mut checkpoint, builtin_activation_identity());
    continuation.result.result = Some(json!({
        "status": "active",
        "capability": "browser_automation",
    }));

    assert!(!checkpoint_continuation_uses_external_mcp_projection(&checkpoint).unwrap());
    let host_committed = crate::conversation_trace::
        conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection(
            &checkpoint,
            &continuation.call,
            &continuation.result,
            Some("assistant-builtin-activation"),
            "Browser automation is active for this task.",
            ConversationHistoryArchiveTraceMetadata::default(),
        )
        .unwrap();
    let observer_setup =
        observer_setup_trace(&checkpoint, &continuation, "assistant-builtin-activation");
    let restored =
        restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
    let runtime_setup = restored.conversation_trace.snapshot();

    assert_eq!(observer_setup.items, host_committed.items);
    assert_eq!(runtime_setup.items, host_committed.items);
    let rendered = serde_json::to_string(&runtime_setup.items).unwrap();
    assert!(rendered.contains("browser_automation"));
    assert!(!rendered.contains("\"type\":\"mcp_tool\""));
    assert!(!rendered.contains("\"external\":true"));
}

#[test]
fn builtin_sensitive_action_id_preserves_one_standard_tool_result_prefix() {
    let tool_name = "browser_evaluate";
    let (mut checkpoint, mut continuation) =
        restorable_checkpoint_fixture_for_pending_tool(tool_name);
    checkpoint.pending_action_id = Some(uuid::Uuid::new_v4().to_string());
    freeze_pending_provenance(&mut checkpoint, builtin_sensitive_identity(tool_name));
    freeze_pending_approval_status(&mut checkpoint, AgentApprovalStatus::Required);
    continuation.call.approval_status = AgentApprovalStatus::Required;
    continuation.result.result = Some(json!({
        "status": "completed",
        "secret": "BUILTIN_SENSITIVE_RESULT_MUST_NOT_PERSIST",
        "structuredContent": {
            "artifacts": [{
                "schemaVersion": 1,
                "artifactId": "browser-artifact:123e4567-e89b-42d3-a456-426614174000",
                "kind": "json",
                "displayName": "storage-state.json",
                "mimeType": "application/json",
                "sizeBytes": 128,
                "createdAt": 1_000,
                "expiresAt": 2_000,
                "lifecycle": "run",
                "owner": "browser_automation",
                "preview": "none"
            }]
        }
    }));

    assert!(!checkpoint_continuation_uses_external_mcp_projection(&checkpoint).unwrap());
    let durable_result =
        crate::tools::builtin_capability_tool_result_persistence_projection(&continuation.result);
    let host_committed = crate::conversation_trace::
        conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection(
            &checkpoint,
            &continuation.call,
            &durable_result,
            Some("assistant-builtin-sensitive"),
            "The approved browser operation completed.",
            ConversationHistoryArchiveTraceMetadata::default(),
        )
        .unwrap();
    let observer_setup =
        observer_setup_trace(&checkpoint, &continuation, "assistant-builtin-sensitive");
    let restored =
        restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
    let runtime_setup = restored.conversation_trace.snapshot();

    assert_eq!(observer_setup.items, host_committed.items);
    assert_eq!(runtime_setup.items, host_committed.items);
    let rendered = serde_json::to_string(&runtime_setup.items).unwrap();
    assert!(rendered.contains("browser-artifact:123e4567-e89b-42d3-a456-426614174000"));
    assert!(!rendered.contains("BUILTIN_SENSITIVE_RESULT_MUST_NOT_PERSIST"));
    assert_eq!(
        runtime_setup
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. }
                    if call_id == &continuation.call.id
            ))
            .count(),
        1
    );
}

#[test]
fn builtin_activation_decisions_are_one_result_of_the_original_tool_call() {
    let call_id = "builtin-activation-decision-call";
    let approval = crate::AgentBuiltinCapabilityActivationApproval {
        action_id: uuid::Uuid::new_v4().to_string(),
        activation_id: uuid::Uuid::new_v4().to_string(),
        run_id: "builtin-activation-decision-run".to_string(),
        call_id: call_id.to_string(),
        capability_id: "browser_automation".to_string(),
        display_name: "Browser automation".to_string(),
        reason: "Open the requested page".to_string(),
        manifest_digest: format!("sha256:{}", "1".repeat(64)),
        policy_revision: 1,
        created_at: 1,
        expires_at: 901,
        approval_status: AgentApprovalStatus::Required,
    };
    let outcomes = [
        (
            "approved_and_succeeded",
            crate::builtin_capability_activation_result(
                &approval,
                crate::CapabilityActivationState::Active,
                None,
            ),
            None,
        ),
        (
            "approved_and_failed",
            AgentToolResult {
                exact_archive_file: None,
                call_id: call_id.to_string(),
                tool: "activate_capability".to_string(),
                ok: false,
                result: Some(json!({
                    "status": "revoked",
                    "capability": "browser_automation",
                })),
                error: Some("fixture activation failure".to_string()),
            },
            None,
        ),
        (
            "rejected_without_reason",
            crate::builtin_capability_activation_rejected_result(&approval, None),
            None,
        ),
        (
            "rejected_with_reason",
            crate::builtin_capability_activation_rejected_result(
                &approval,
                Some("Do not use the browser for this task"),
            ),
            Some("Do not use the browser for this task"),
        ),
    ];

    for (label, result, expected_feedback) in outcomes {
        assert_eq!(result.call_id, call_id, "{label}");
        assert_eq!(result.tool, "activate_capability", "{label}");
        assert_eq!(
            result
                .result
                .as_ref()
                .and_then(|value| value.get("userFeedback"))
                .and_then(serde_json::Value::as_str),
            expected_feedback,
            "{label} must keep optional rejection guidance inside the ToolResult"
        );
        if label.starts_with("rejected") {
            assert!(result.ok, "{label} is a normal tool settlement");
        }

        let (mut checkpoint, mut continuation) =
            restorable_checkpoint_fixture_for_pending_tool("activate_capability");
        let original_call_id = checkpoint.pending_tool_call_id.clone();
        checkpoint.pending_action_id = Some(approval.action_id.clone());
        freeze_pending_provenance(&mut checkpoint, builtin_activation_identity());
        continuation.call.id = original_call_id.clone();
        continuation.result = AgentToolResult {
            call_id: original_call_id.clone(),
            ..result
        };
        continuation.call.approval_status = if label.starts_with("rejected") {
            AgentApprovalStatus::Rejected
        } else {
            AgentApprovalStatus::Approved
        };

        let restored =
            restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
        let items = restored.conversation_trace.snapshot().items;
        assert_eq!(
            items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolCall { call_id, .. }
                        if call_id == &original_call_id
                ))
                .count(),
            1,
            "{label} must retain one original ToolCall"
        );
        assert_eq!(
            items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolResult { call_id, .. }
                        if call_id == &original_call_id
                ))
                .count(),
            1,
            "{label} must settle with one ToolResult"
        );
    }
}

#[test]
fn external_mcp_continuation_still_uses_its_content_free_projection() {
    let (mut checkpoint, mut continuation) =
        restorable_checkpoint_fixture_for_pending_tool("mcp__fixture__read");
    checkpoint.pending_action_id = Some(uuid::Uuid::new_v4().to_string());
    continuation.result.result = Some(json!({
        "status": "completed",
        "content": "external-result-canary-that-must-not-persist",
        "isError": false,
    }));

    assert!(checkpoint_continuation_uses_external_mcp_projection(&checkpoint).unwrap());
    let durable_result = crate::tools::mcp_tool_result_persistence_projection(&continuation.result);
    let host_committed = crate::conversation_trace::
        conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection(
            &checkpoint,
            &continuation.call,
            &durable_result,
            Some("assistant-external-mcp"),
            "External MCP tool completed.",
            ConversationHistoryArchiveTraceMetadata::default(),
        )
        .unwrap();
    let observer_setup = observer_setup_trace(&checkpoint, &continuation, "assistant-external-mcp");
    let restored =
        restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
    let runtime_setup = restored.conversation_trace.snapshot();
    let rendered = serde_json::to_string(&runtime_setup.items).unwrap();

    assert_eq!(observer_setup.items, host_committed.items);
    assert_eq!(runtime_setup.items, host_committed.items);
    assert!(rendered.contains("\"type\":\"mcp_tool\""));
    assert!(rendered.contains("\"external\":true"));
    assert!(!rendered.contains("external-result-canary-that-must-not-persist"));
    assert_eq!(
        runtime_setup
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. }
                    if call_id == &continuation.call.id
            ))
            .count(),
        1
    );
}

#[test]
fn continuation_projection_rejects_untrusted_or_mismatched_approval_identity() {
    let trusted_builtin = AgentToolIdentity::Builtin {
        tool_name: "read_file".to_string(),
    };
    let cases = [
        (
            "unregistered",
            AgentToolIdentity::Unregistered {
                tool_name: "activate_capability".to_string(),
            },
            true,
            true,
        ),
        (
            "wrong_extension",
            AgentToolIdentity::RuntimeExtension {
                extension_id: "another.extension".to_string(),
                tool_name: "activate_capability".to_string(),
            },
            true,
            true,
        ),
        (
            "ordinary_builtin_with_action",
            trusted_builtin.clone(),
            true,
            true,
        ),
        (
            "ordinary_builtin_without_action",
            trusted_builtin,
            false,
            false,
        ),
        (
            "activation_without_action",
            builtin_activation_identity(),
            false,
            true,
        ),
    ];

    for (label, provenance, has_action_id, should_fail) in cases {
        let (mut checkpoint, _) =
            restorable_checkpoint_fixture_for_pending_tool("activate_capability");
        checkpoint.pending_action_id = has_action_id.then(|| uuid::Uuid::new_v4().to_string());
        freeze_pending_provenance(&mut checkpoint, provenance);

        let result = checkpoint_continuation_uses_external_mcp_projection(&checkpoint);
        assert_eq!(result.is_err(), should_fail, "{label}");
        if !should_fail {
            assert!(!result.unwrap(), "{label}");
        }
    }

    let (mut mcp_without_action, _) =
        restorable_checkpoint_fixture_for_pending_tool("mcp__fixture__read");
    mcp_without_action.pending_action_id = None;
    assert!(checkpoint_continuation_uses_external_mcp_projection(&mcp_without_action).is_err());

    let (mut automatic_builtin_with_action, _) =
        restorable_checkpoint_fixture_for_pending_tool("browser_navigate");
    automatic_builtin_with_action.pending_action_id = Some(uuid::Uuid::new_v4().to_string());
    freeze_pending_provenance(
        &mut automatic_builtin_with_action,
        builtin_sensitive_identity("browser_navigate"),
    );
    freeze_pending_approval_status(
        &mut automatic_builtin_with_action,
        AgentApprovalStatus::NotRequired,
    );
    assert!(
        checkpoint_continuation_uses_external_mcp_projection(&automatic_builtin_with_action)
            .is_err()
    );
}

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
        name: "apply_patch".to_string(),
        args: apply_patch_args(json!({
            "action": "commit",
            "transactionId": "transaction-provider-pending",
            "expectedDraftRevision": 1
        })),
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
fn builtin_sensitive_approval_defers_private_siblings_without_persisting_any_arguments() {
    let secret = "BUILTIN_PENDING_PASSWORD_COOKIE_STORAGE_FILE_HANDLE_SECRET";
    let pending = LlmToolCall {
        id: canonical_test_call_id(0, "provider-builtin-sensitive-pending"),
        name: "browser_evaluate".to_string(),
        args: json!({
            "function": format!("() => localStorage.getItem('{secret}')"),
            "password": secret,
            "cookie": secret,
            "call_reason": "Read the reviewed page state.",
        }),
    };
    let mut batch = ToolCallBatch::from_model_response(
        "checkpoint-validation-run",
        0,
        String::new(),
        vec![
            pending.clone(),
            LlmToolCall {
                id: canonical_test_call_id(1, "provider-builtin-ordinary"),
                name: "browser_click".to_string(),
                args: json!({"ref": secret, "call_reason": "Continue the reviewed flow."}),
            },
            LlmToolCall {
                id: canonical_test_call_id(2, "provider-builtin-sensitive-sibling"),
                name: "browser_file_upload".to_string(),
                args: json!({
                    "paths": [format!("browser-file:{secret}")],
                    "call_reason": "Upload the reviewed file.",
                }),
            },
            LlmToolCall {
                id: canonical_test_call_id(3, "provider-safe-sibling"),
                name: "read_file".to_string(),
                args: json!({"path": "report.txt"}),
            },
        ],
        false,
        |call| {
            let private_builtin = call.name.starts_with("browser_");
            (
                LlmToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: if private_builtin {
                        json!({})
                    } else {
                        call.args.clone()
                    },
                },
                AgentToolCallCheckpointPersistence::Allowed,
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

    let deferred = batch.defer_external_calls(|queued| {
        queued.checkpoint_persistence != AgentToolCallCheckpointPersistence::Allowed
            || queued.checkpoint_call.args != queued.call.args
    });
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

    let mut trace = ConversationTraceRecorder::default();
    record_current_test_tool_call(
        &mut trace,
        &batch,
        &AgentToolCall {
            id: pending.id.clone(),
            tool: pending.name.clone(),
            args: json!({}),
            approval_status: AgentApprovalStatus::Required,
            reason: Some("Read the reviewed page state.".to_string()),
        },
    );
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
    assert!(!rendered.contains("browser_click"));
    assert!(!rendered.contains("browser_file_upload"));
    assert!(!rendered.contains("function"));
    assert!(!rendered.contains("password"));
    assert!(!rendered.contains("cookie"));
    assert!(!rendered.contains("browser-file:"));
}

#[test]
fn current_checkpoint_schema_round_trips_and_rejects_missing_or_extra_fields() {
    let (checkpoint, _) = restorable_checkpoint_fixture();
    let canonical = serde_json::to_value(&checkpoint).unwrap();
    let decoded: AgentRunCheckpoint = serde_json::from_value(canonical.clone()).unwrap();
    assert_eq!(decoded, checkpoint);

    for field in [
        "pauseReason",
        "deferredExternalToolCallCount",
        "providerContinuationRefs",
        "runContext",
        "collaborationRunSnapshot",
        "pendingActionId",
        "fileChangeRunGrantRef",
        "pendingFileObservation",
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
    assert!(canonical["pendingActionId"].is_null());
    assert!(canonical["fileChangeRunGrantRef"].is_null());
    assert!(canonical["pendingFileObservation"].is_null());

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
