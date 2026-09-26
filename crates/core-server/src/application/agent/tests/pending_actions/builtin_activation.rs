use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::*;

#[derive(Clone)]
struct AutoActivationTestProvider {
    manifest: mycopilot_core::BuiltinCapabilityManifest,
    grant: Arc<Mutex<Option<mycopilot_core::CapabilityGrant>>>,
    approvals: Arc<std::sync::atomic::AtomicUsize>,
    revocations: Arc<std::sync::atomic::AtomicUsize>,
    cancel_on_approve: Arc<Mutex<Option<AgentCancellationToken>>>,
}

impl mycopilot_core::BuiltinCapabilityProvider for AutoActivationTestProvider {
    fn manifests(&self) -> AgentResult<Vec<mycopilot_core::BuiltinCapabilityManifest>> {
        Ok(vec![self.manifest.clone()])
    }

    fn policy(
        &self,
        _capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<mycopilot_core::BuiltinCapabilityPolicy> {
        Ok(mycopilot_core::BuiltinCapabilityPolicy {
            user_allowed: true,
            revision: 1,
        })
    }

    fn grant(
        &self,
        _run_id: &str,
        _capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<Option<mycopilot_core::CapabilityGrant>> {
        Ok(self.grant.lock().unwrap().clone())
    }

    fn approve_activation(
        &self,
        approval: &mycopilot_core::AgentBuiltinCapabilityActivationApproval,
    ) -> AgentResult<mycopilot_core::CapabilityGrant> {
        self.approvals
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let grant = mycopilot_core::CapabilityGrant {
            run_id: approval.run_id.clone(),
            capability_id: mycopilot_core::BuiltinCapabilityId::parse(
                approval.capability_id.clone(),
            )?,
            activation_id: mycopilot_core::CapabilityActivationId::parse(
                approval.activation_id.clone(),
            )?,
            manifest_digest: approval.manifest_digest.clone(),
            upstream_catalog_digest: self
                .manifest
                .provider_contract
                .upstream_catalog_digest
                .clone(),
            provider_policy_digest: self.manifest.provider_contract.policy_digest.clone(),
            policy_revision: approval.policy_revision,
            created_at: approval.created_at,
            expires_at: approval
                .created_at
                .saturating_add(mycopilot_core::BUILTIN_CAPABILITY_GRANT_TTL_SECONDS),
        };
        *self.grant.lock().unwrap() = Some(grant.clone());
        if let Some(cancellation) = self.cancel_on_approve.lock().unwrap().take() {
            cancellation.cancel();
        }
        Ok(grant)
    }

    fn revoke_activation(
        &self,
        _activation_id: &mycopilot_core::CapabilityActivationId,
        _action_id: &str,
    ) -> AgentResult<()> {
        self.revocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.grant.lock().unwrap() = None;
        Ok(())
    }

    fn revoke_grants(
        &self,
        _capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<()> {
        *self.grant.lock().unwrap() = None;
        Ok(())
    }

    fn invoke_authorized<'a>(
        &'a self,
        _invocation: mycopilot_core::BuiltinCapabilityInvocation,
        _expected_grant: mycopilot_core::CapabilityGrant,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::BuiltinCapabilityFuture<'a, Value> {
        Box::pin(async { Err(AgentError::new("not used by activation test")) })
    }
}

fn auto_activation_test_provider() -> AutoActivationTestProvider {
    let manifest = mycopilot_core::BuiltinCapabilityManifest::new(
        mycopilot_core::BuiltinCapabilityDescriptor {
            id: mycopilot_core::BuiltinCapabilityId::parse("browser_automation").unwrap(),
            display_name: "Browser automation".to_string(),
            description: "Control the reviewed browser".to_string(),
        },
        "builtin.browser_automation.mcp",
        "fixture-v1",
        vec![mycopilot_core::BuiltinCapabilityToolDescriptor::new(
            "browser_snapshot",
            "browser_snapshot",
            "Read the current page",
            json!({"type":"object", "additionalProperties": false}),
            mycopilot_core::AgentToolSafety::ReadOnly,
            false,
        )
        .unwrap()],
    )
    .unwrap();
    AutoActivationTestProvider {
        manifest,
        grant: Arc::new(Mutex::new(None)),
        approvals: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        revocations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        cancel_on_approve: Arc::new(Mutex::new(None)),
    }
}

fn auto_activation_test_input() -> AgentChatInput {
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.context = Some(mycopilot_core::AgentRunContext {
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: mycopilot_core::AgentPermissions {
            builtin_execution: mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove,
            ..mycopilot_core::AgentPermissions::default()
        },
        collaboration_identity: None,
    });
    input
}

fn auto_activation_test_action(
    provider: &AutoActivationTestProvider,
    run_id: &str,
) -> AgentProposedAction {
    let now = u64::try_from(mycopilot_core::storage::now_ms()).unwrap() / 1_000;
    AgentProposedAction::BuiltinCapabilityActivation {
        approval: Box::new(mycopilot_core::AgentBuiltinCapabilityActivationApproval {
            action_id: uuid::Uuid::new_v4().to_string(),
            activation_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            call_id: "auto-activation-call".to_string(),
            capability_id: "browser_automation".to_string(),
            display_name: "Browser automation".to_string(),
            reason: "Open the reviewed browser".to_string(),
            manifest_digest: provider.manifest.manifest_digest.clone(),
            policy_revision: 1,
            created_at: now,
            expires_at: now.saturating_add(900),
            approval_status: AgentApprovalStatus::Approved,
        }),
    }
}

#[test]
fn auto_activation_has_durable_non_replayable_receipt_and_cancellation_revokes_grant() {
    for cancel_during_approval in [false, true] {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("auto-activation.sqlite")).unwrap());
        let provider = auto_activation_test_provider();
        let runtime =
            mycopilot_core::BuiltinCapabilityRuntime::new(Arc::new(provider.clone())).unwrap();
        let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
            .with_builtin_capabilities(runtime);
        let run_id = if cancel_during_approval {
            "auto-activation-cancel-run"
        } else {
            "auto-activation-success-run"
        };
        let action = auto_activation_test_action(&provider, run_id);
        let input = auto_activation_test_input();
        let cancellation = AgentCancellationToken::new();
        if cancel_during_approval {
            *provider.cancel_on_approve.lock().unwrap() = Some(cancellation.clone());
        }
        let context =
            AutoApprovedActionContext::new(input.clone(), run_id.to_string(), None, None, None);
        let result = service
            .execute_auto_approved_action(context, action.clone(), cancellation)
            .unwrap();
        assert_eq!(
            result.ok, !cancel_during_approval,
            "cancellation after grant minting must be reflected in the durable result"
        );
        assert_eq!(
            provider.approvals.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            provider.grant.lock().unwrap().is_some(),
            !cancel_during_approval
        );
        assert_eq!(
            provider
                .revocations
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(cancel_during_approval)
        );

        let replay = service.execute_auto_approved_action(
            AutoApprovedActionContext::new(input, run_id.to_string(), None, None, None),
            action,
            AgentCancellationToken::new(),
        );
        assert!(
            replay.is_err(),
            "a durable activation receipt must never replay"
        );
        assert_eq!(
            provider.approvals.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }
}

#[test]
fn builtin_capability_pending_binding_requires_exact_action_and_call_ids() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open(&fixture.path().join("builtin-capability-binding.sqlite")).unwrap(),
    );
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let run_id = "builtin-capability-binding-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let activation_id = uuid::Uuid::new_v4().to_string();
    let call = AgentToolCall {
        id: "builtin-capability-call".to_string(),
        tool: "activate_capability".to_string(),
        args: json!({
            "capability": "browser_automation",
            "reason": "Inspect the task page"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::BuiltinCapabilityActivation {
        approval: Box::new(mycopilot_core::AgentBuiltinCapabilityActivationApproval {
            action_id: action_id.clone(),
            activation_id,
            run_id: run_id.to_string(),
            call_id: call.id.clone(),
            capability_id: "browser_automation".to_string(),
            display_name: "Browser automation".to_string(),
            reason: "Inspect the task page".to_string(),
            manifest_digest: format!("sha256:{}", "1".repeat(64)),
            policy_revision: 1,
            created_at: 1,
            expires_at: 901,
            approval_status: AgentApprovalStatus::Required,
        }),
    };
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut input);
    input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&action_id),
        &call,
        AgentToolIdentity::RuntimeExtension {
            extension_id: "builtin.capabilities".to_string(),
            tool_name: "activate_capability".to_string(),
        },
    ));

    assert!(pending_action_binding_matches(
        run_id, None, &action, &input
    ));

    {
        let checkpoint_call = input
            .resume_checkpoint
            .as_mut()
            .unwrap()
            .context_items
            .iter_mut()
            .flat_map(|item| item.tool_calls.iter_mut())
            .find(|candidate| candidate.id == call.id)
            .unwrap();
        checkpoint_call.args["reason"] = json!("A different model-authored reason");
    }
    assert!(!pending_action_binding_matches(
        run_id, None, &action, &input
    ));
    {
        let checkpoint_call = input
            .resume_checkpoint
            .as_mut()
            .unwrap()
            .context_items
            .iter_mut()
            .flat_map(|item| item.tool_calls.iter_mut())
            .find(|candidate| candidate.id == call.id)
            .unwrap();
        checkpoint_call.args = call.args.clone();
        checkpoint_call.args["capability"] = json!("another_capability");
    }
    assert!(!pending_action_binding_matches(
        run_id, None, &action, &input
    ));
    {
        let checkpoint_call = input
            .resume_checkpoint
            .as_mut()
            .unwrap()
            .context_items
            .iter_mut()
            .flat_map(|item| item.tool_calls.iter_mut())
            .find(|candidate| candidate.id == call.id)
            .unwrap();
        checkpoint_call.args = call.args.clone();
        checkpoint_call.args["unexpected"] = json!(true);
    }
    assert!(!pending_action_binding_matches(
        run_id, None, &action, &input
    ));
    input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .context_items
        .iter_mut()
        .flat_map(|item| item.tool_calls.iter_mut())
        .find(|candidate| candidate.id == call.id)
        .unwrap()
        .args = call.args.clone();
    assert!(pending_action_binding_matches(
        run_id, None, &action, &input
    ));

    input.resume_checkpoint.as_mut().unwrap().pending_action_id =
        Some(uuid::Uuid::new_v4().to_string());
    assert!(!pending_action_binding_matches(
        run_id, None, &action, &input
    ));

    input.resume_checkpoint.as_mut().unwrap().pending_action_id = Some(action_id.clone());
    let mut drifted_action = action.clone();
    let AgentProposedAction::BuiltinCapabilityActivation { approval } = &mut drifted_action else {
        unreachable!();
    };
    approval.run_id = "different-run".to_string();
    assert!(!pending_action_binding_matches(
        run_id,
        None,
        &drifted_action,
        &input
    ));

    let storage_id = pending_action_storage_id(run_id, &action_id);
    storage
        .upsert_agent_action_audit(AgentActionAuditRecord {
            action_id: storage_id.clone(),
            run_id: "conflicting-audit-owner".to_string(),
            conversation_id: None,
            assistant_message_id: None,
            action_type: "builtin_capability_activation".to_string(),
            tool_name: "activate_capability".to_string(),
            decision: None,
            status: "pending".to_string(),
            action_json: "{}".to_string(),
            file_change_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 1,
            decided_at: None,
            completed_at: None,
            effective_permissions_json: None,
            path_scope: None,
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("manual_pending".to_string()),
        })
        .unwrap();
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let failed = service.store_pending_action(
        run_id,
        "builtin-capability-binding-conversation",
        "builtin-capability-binding-assistant",
        action.clone(),
        input.clone(),
    );
    assert!(
        failed.is_err(),
        "a conflicting second write must abort publication"
    );
    assert!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .is_none(),
        "the pending insert must roll back with the conflicting audit"
    );
    rusqlite::Connection::open(fixture.path().join("builtin-capability-binding.sqlite"))
        .unwrap()
        .execute(
            "DELETE FROM agent_action_audit WHERE action_id = ?1",
            [&storage_id],
        )
        .unwrap();

    assert!(service
        .store_pending_action(
            run_id,
            "builtin-capability-binding-conversation",
            "builtin-capability-binding-assistant",
            action,
            input,
        )
        .unwrap());
    drop(service);

    let restarted = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let pending = restarted.list_pending_actions();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].action_id, action_id);
    assert_eq!(pending[0].tool_call_id.as_deref(), Some(call.id.as_str()));
    assert_eq!(pending[0].action_type, "builtin_capability_activation");
    let persisted_pair: (i64, i64) =
        rusqlite::Connection::open(fixture.path().join("builtin-capability-binding.sqlite"))
            .unwrap()
            .query_row(
                "SELECT
             (SELECT COUNT(*) FROM agent_pending_actions WHERE action_id = ?1),
             (SELECT COUNT(*) FROM agent_action_audit WHERE action_id = ?1 AND status = 'pending')",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(persisted_pair, (1, 1));
}

#[tokio::test]
async fn builtin_capability_approval_waits_past_its_proposal_window_and_can_still_be_approved() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("builtin-capability-expiry.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let run_id = "builtin-capability-expiry-run";
    let conversation_id = "builtin-capability-expiry-conversation";
    let assistant_message_id = "builtin-capability-expiry-assistant";
    let action_id = uuid::Uuid::new_v4().to_string();
    let expiry_now_ms = mycopilot_core::storage::now_ms();
    let expiry_now_seconds = u64::try_from(expiry_now_ms).unwrap() / 1_000;
    let provider = auto_activation_test_provider();
    let runtime =
        mycopilot_core::BuiltinCapabilityRuntime::new(Arc::new(provider.clone())).unwrap();
    let call = AgentToolCall {
        id: "builtin-capability-expiry-call".to_string(),
        tool: "activate_capability".to_string(),
        args: json!({
            "capability": "browser_automation",
            "reason": "Inspect the task page"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::BuiltinCapabilityActivation {
        approval: Box::new(mycopilot_core::AgentBuiltinCapabilityActivationApproval {
            action_id: action_id.clone(),
            activation_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            call_id: call.id.clone(),
            capability_id: "browser_automation".to_string(),
            display_name: "Browser automation".to_string(),
            reason: "Inspect the task page".to_string(),
            manifest_digest: provider.manifest.manifest_digest.clone(),
            policy_revision: 1,
            created_at: expiry_now_seconds.saturating_sub(901),
            expires_at: expiry_now_seconds.saturating_sub(1),
            approval_status: AgentApprovalStatus::Required,
        }),
    };
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut input);
    input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&action_id),
        &call,
        AgentToolIdentity::RuntimeExtension {
            extension_id: "builtin.capabilities".to_string(),
            tool_name: "activate_capability".to_string(),
        },
    ));
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    input.context = Some(run_context.clone());
    input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
        .with_builtin_capabilities(runtime)
        .with_mcp_approval_clock(move || expiry_now_ms);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::RuntimeExtension {
            extension_id: "builtin.capabilities".to_string(),
            tool_name: "activate_capability".to_string(),
        },
        expiry_now_ms.saturating_sub(1_000),
    );

    assert!(service
        .store_pending_action(run_id, conversation_id, assistant_message_id, action, input,)
        .unwrap());
    assert_eq!(service.list_pending_actions().len(), 1);
    let pre_expiry_trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(pre_expiry_trace.run_id, run_id);
    assert_eq!(pre_expiry_trace.conversation_id, conversation_id);
    assert_eq!(pre_expiry_trace.assistant_message_id, assistant_message_id);
    assert_eq!(
        pre_expiry_trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    assert_eq!(
        service
            .reconcile_expired_builtin_capability_approvals()
            .unwrap(),
        0
    );
    assert_eq!(service.list_pending_actions().len(), 1);
    assert_eq!(
        service
            .reconcile_expired_builtin_capability_approvals()
            .unwrap(),
        0,
        "the periodic reconciler must leave human approval pending"
    );

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let pending: (String, String, String) = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status, action_json, agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(pending.0, "pending");
    assert_ne!(pending.1, "{}");
    assert_ne!(pending.2, "{}");
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        0,
        "waiting past the proposal window must not synthesize a failure result"
    );

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let output = service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
        .unwrap();
    assert_eq!(output.status, "approved");
    assert_eq!(
        provider.approvals.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let grant = provider.grant.lock().unwrap().clone().unwrap();
    assert_eq!(grant.created_at, expiry_now_seconds);
    assert!(grant.created_at > expiry_now_seconds.saturating_sub(1));
}
