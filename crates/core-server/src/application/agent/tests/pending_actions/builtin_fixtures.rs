use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::*;

#[derive(Clone)]
pub(super) struct AutoSensitiveTestProvider {
    manifest: mycopilot_core::BuiltinCapabilityManifest,
    capability_grant: mycopilot_core::CapabilityGrant,
    storage: Arc<StorageService>,
    pub(super) invocations: Arc<std::sync::atomic::AtomicUsize>,
    pub(super) revocations: Arc<std::sync::atomic::AtomicUsize>,
}

impl mycopilot_core::BuiltinCapabilityProvider for AutoSensitiveTestProvider {
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
        run_id: &str,
        capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<Option<mycopilot_core::CapabilityGrant>> {
        Ok((self.capability_grant.run_id == run_id
            && self.capability_grant.capability_id == *capability_id)
            .then(|| self.capability_grant.clone()))
    }

    fn approve_activation(
        &self,
        _approval: &mycopilot_core::AgentBuiltinCapabilityActivationApproval,
    ) -> AgentResult<mycopilot_core::CapabilityGrant> {
        Err(AgentError::new("not used by sensitive auto test"))
    }

    fn revoke_activation(
        &self,
        _activation_id: &mycopilot_core::CapabilityActivationId,
        _action_id: &str,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn revoke_grants(
        &self,
        _capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn approve_builtin_mcp_tool(
        &self,
        approval: &mycopilot_core::AgentBuiltinMcpToolApproval,
    ) -> AgentResult<mycopilot_core::BuiltinMcpToolGrant> {
        let identity = &approval.identity;
        Ok(mycopilot_core::BuiltinMcpToolGrant {
            grant_id: uuid::Uuid::new_v4().to_string(),
            approval_id: identity.approval_id.clone(),
            run_id: identity.run_id.clone(),
            call_id: identity.call_id.clone(),
            capability_id: mycopilot_core::BuiltinCapabilityId::parse(
                identity.capability_id.clone(),
            )?,
            capability_activation_id: mycopilot_core::CapabilityActivationId::parse(
                identity.capability_activation_id.clone(),
            )?,
            managed_mcp_id: identity.managed_mcp_id.clone(),
            package_name: identity.package_name.clone(),
            package_version: identity.package_version.clone(),
            upstream_catalog_digest: identity.upstream_catalog_digest.clone(),
            manifest_digest: identity.manifest_digest.clone(),
            policy_digest: identity.policy_digest.clone(),
            policy_revision: identity.policy_revision,
            tool_id: identity.tool_id.clone(),
            raw_name: identity.raw_name.clone(),
            model_name: identity.model_name.clone(),
            upstream_schema_digest: identity.upstream_schema_digest.clone(),
            host_overlay_digest: identity.host_overlay_digest.clone(),
            host_input_schema_digest: identity.host_input_schema_digest.clone(),
            arguments_digest: identity.arguments_digest.clone(),
            resource_scope_digest: identity.resource_scope_digest.clone(),
            target_binding_id: None,
            target_binding_digest: None,
            origin: identity.origin.clone(),
            risk_kinds: approval.risk_kinds.clone(),
            created_at: approval.created_at,
            expires_at: approval.expires_at,
        })
    }

    fn revoke_builtin_mcp_tool_grant(
        &self,
        _grant_id: &str,
        _approval_id: &str,
    ) -> AgentResult<()> {
        self.revocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn invoke_approved_builtin_mcp_tool<'a>(
        &'a self,
        approval: mycopilot_core::AgentBuiltinMcpToolApproval,
        _grant: mycopilot_core::BuiltinMcpToolGrant,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::BuiltinCapabilityFuture<'a, Value> {
        Box::pin(async move {
            let storage_id =
                pending_action_storage_id(&approval.identity.run_id, &approval.identity.action_id);
            let record = self
                .storage
                .get_pending_agent_action(&storage_id)?
                .ok_or_else(|| AgentError::new("missing durable sensitive auto journal"))?;
            if record.status != "executing" {
                return Err(AgentError::new(
                    "sensitive invocation ran before the durable executing claim",
                ));
            }
            self.invocations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(json!({"status": "ok"}))
        })
    }

    fn invoke_authorized<'a>(
        &'a self,
        _invocation: mycopilot_core::BuiltinCapabilityInvocation,
        _expected_grant: mycopilot_core::CapabilityGrant,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::BuiltinCapabilityFuture<'a, Value> {
        Box::pin(async { Err(AgentError::new("not used by sensitive auto test")) })
    }
}

pub(super) fn auto_sensitive_test_fixture(
    storage: Arc<StorageService>,
    run_id: &str,
    call_id: &str,
) -> (
    mycopilot_core::BuiltinCapabilityRuntime,
    AutoSensitiveTestProvider,
    mycopilot_core::AgentBuiltinMcpToolApproval,
    AgentChatInput,
) {
    let descriptor = mycopilot_core::BuiltinCapabilityToolDescriptor::new(
        "browser_evaluate",
        "browser_evaluate",
        "Evaluate a reviewed script",
        json!({
            "type": "object",
            "properties": {"function": {"type": "string"}},
            "required": ["function"],
            "additionalProperties": false
        }),
        mycopilot_core::AgentToolSafety::RequiresApproval,
        false,
    )
    .unwrap()
    .with_builtin_approval_policy(
        mycopilot_core::BuiltinMcpToolApprovalMode::Always,
        vec![mycopilot_core::BuiltinMcpToolRiskKind::PageScriptExecution],
    )
    .unwrap();
    let manifest = mycopilot_core::BuiltinCapabilityManifest::new(
        mycopilot_core::BuiltinCapabilityDescriptor {
            id: mycopilot_core::BuiltinCapabilityId::parse("browser_automation").unwrap(),
            display_name: "Browser automation".to_string(),
            description: "Control the reviewed browser".to_string(),
        },
        "builtin.browser_automation.mcp",
        "fixture-v1",
        vec![descriptor.clone()],
    )
    .unwrap();
    let now = u64::try_from(mycopilot_core::storage::now_ms()).unwrap() / 1_000;
    let activation_id = mycopilot_core::CapabilityActivationId::generate();
    let capability_grant = mycopilot_core::CapabilityGrant {
        run_id: run_id.to_string(),
        capability_id: manifest.descriptor.id.clone(),
        activation_id: activation_id.clone(),
        manifest_digest: manifest.manifest_digest.clone(),
        upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
        provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
        policy_revision: 1,
        created_at: now,
        expires_at: now.saturating_add(mycopilot_core::BUILTIN_CAPABILITY_GRANT_TTL_SECONDS),
    };
    let provider = AutoSensitiveTestProvider {
        manifest: manifest.clone(),
        capability_grant,
        storage: Arc::clone(&storage),
        invocations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        revocations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let runtime =
        mycopilot_core::BuiltinCapabilityRuntime::new(Arc::new(provider.clone())).unwrap();
    let action_id = uuid::Uuid::new_v4().to_string();
    let approval = mycopilot_core::AgentBuiltinMcpToolApproval {
        schema_version: mycopilot_core::BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION,
        identity: mycopilot_core::BuiltinMcpToolApprovalIdentity {
            action_id: action_id.clone(),
            approval_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            call_id: call_id.to_string(),
            capability_id: manifest.descriptor.id.as_str().to_string(),
            capability_activation_id: activation_id.as_str().to_string(),
            managed_mcp_id: manifest.managed_mcp_id.clone(),
            package_name: manifest.provider_contract.package_name.clone(),
            package_version: manifest.provider_contract.package_version.clone(),
            upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            policy_digest: manifest.provider_contract.policy_digest.clone(),
            policy_revision: 1,
            tool_id: descriptor.tool_id.clone(),
            raw_name: descriptor.raw_name.clone(),
            model_name: descriptor.model_name.clone(),
            upstream_schema_digest: descriptor.upstream_schema_digest.clone(),
            host_overlay_digest: descriptor.host_overlay_digest.clone(),
            host_input_schema_digest: descriptor.schema_digest.clone(),
            arguments_digest: format!("sha256:{}", "7".repeat(64)),
            resource_scope_digest: format!("sha256:{}", "8".repeat(64)),
            origin: Some("https://mail.example.test".to_string()),
        },
        capability_display_name: manifest.descriptor.display_name.clone(),
        tool_display_name: descriptor.model_name.clone(),
        call_reason: "Run the reviewed page script.".to_string(),
        operation_category: "page_script_execution".to_string(),
        resource_summary: mycopilot_core::BuiltinMcpToolResourceSummary {
            scope: "managed_surface".to_string(),
            display_name: "Current managed page".to_string(),
            file_basenames: Vec::new(),
            origin: Some("https://mail.example.test".to_string()),
        },
        risk_kinds: vec![mycopilot_core::BuiltinMcpToolRiskKind::PageScriptExecution],
        created_at: now,
        expires_at: now.saturating_add(900),
        approval_status: AgentApprovalStatus::Approved,
    };
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: descriptor.model_name.clone(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Approved,
        reason: Some(approval.call_reason.clone()),
    };
    let provenance = AgentToolIdentity::BuiltinCapability {
        capability_id: approval.identity.capability_id.clone().into(),
        managed_mcp_id: approval.identity.managed_mcp_id.clone().into(),
        package_name: approval.identity.package_name.clone().into(),
        package_version: approval.identity.package_version.clone().into(),
        upstream_catalog_digest: approval.identity.upstream_catalog_digest.clone().into(),
        policy_digest: approval.identity.policy_digest.clone().into(),
        manifest_digest: approval.identity.manifest_digest.clone().into(),
        tool_id: approval.identity.tool_id.clone().into(),
        raw_name: approval.identity.raw_name.clone().into(),
        model_name: approval.identity.model_name.clone().into(),
        upstream_schema_digest: approval.identity.upstream_schema_digest.clone().into(),
        host_overlay_digest: approval.identity.host_overlay_digest.clone().into(),
        host_input_schema_digest: approval.identity.host_input_schema_digest.clone().into(),
    };
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": {"imageInput": false},
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut input);
    let run_context = mycopilot_core::AgentRunContext {
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: mycopilot_core::AgentPermissions {
            builtin_execution: mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove,
            ..mycopilot_core::AgentPermissions::default()
        },
        collaboration_identity: None,
    };
    input.context = Some(run_context.clone());
    input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&action_id),
        &call,
        provenance,
    ));
    input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    (runtime, provider, approval, input)
}

pub(super) fn store_builtin_sensitive_test_pending(
    service: &AgentService,
    storage: &StorageService,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    call_id: &str,
) -> (String, AgentToolCall) {
    let now_ms = mycopilot_core::storage::now_ms();
    let now_seconds = u64::try_from(now_ms).unwrap() / 1_000;
    let action_id = uuid::Uuid::new_v4().to_string();
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: "browser_evaluate".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: Some("Run the reviewed page script.".to_string()),
    };
    let approval = mycopilot_core::AgentBuiltinMcpToolApproval {
        schema_version: mycopilot_core::BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION,
        identity: mycopilot_core::BuiltinMcpToolApprovalIdentity {
            action_id: action_id.clone(),
            approval_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            call_id: call.id.clone(),
            capability_id: "browser_automation".to_string(),
            capability_activation_id: uuid::Uuid::new_v4().to_string(),
            managed_mcp_id: "builtin.browser_automation.mcp".to_string(),
            package_name: "@playwright/mcp".to_string(),
            package_version: "0.0.79".to_string(),
            upstream_catalog_digest: format!("sha256:{}", "1".repeat(64)),
            manifest_digest: format!("sha256:{}", "2".repeat(64)),
            policy_digest: format!("sha256:{}", "3".repeat(64)),
            policy_revision: 1,
            tool_id: "browser_evaluate".to_string(),
            raw_name: "browser_evaluate".to_string(),
            model_name: "browser_evaluate".to_string(),
            upstream_schema_digest: format!("sha256:{}", "4".repeat(64)),
            host_overlay_digest: format!("sha256:{}", "5".repeat(64)),
            host_input_schema_digest: format!("sha256:{}", "6".repeat(64)),
            arguments_digest: format!("sha256:{}", "7".repeat(64)),
            resource_scope_digest: format!("sha256:{}", "8".repeat(64)),
            origin: Some("https://mail.example.test".to_string()),
        },
        capability_display_name: "Browser automation".to_string(),
        tool_display_name: "Evaluate page script".to_string(),
        call_reason: "Run the reviewed page script.".to_string(),
        operation_category: "page_script_execution".to_string(),
        resource_summary: mycopilot_core::BuiltinMcpToolResourceSummary {
            scope: "page_script_execution".to_string(),
            display_name: "Current managed page".to_string(),
            file_basenames: Vec::new(),
            origin: Some("https://mail.example.test".to_string()),
        },
        risk_kinds: vec![mycopilot_core::BuiltinMcpToolRiskKind::PageScriptExecution],
        created_at: now_seconds,
        expires_at: now_seconds.saturating_add(900),
        approval_status: AgentApprovalStatus::Required,
    };
    let action = AgentProposedAction::BuiltinMcpToolApproval {
        approval: Box::new(approval),
    };
    let provenance = AgentToolIdentity::BuiltinCapability {
        capability_id: "browser_automation".into(),
        managed_mcp_id: "builtin.browser_automation.mcp".into(),
        package_name: "@playwright/mcp".into(),
        package_version: "0.0.79".into(),
        upstream_catalog_digest: format!("sha256:{}", "1".repeat(64)).into(),
        policy_digest: format!("sha256:{}", "3".repeat(64)).into(),
        manifest_digest: format!("sha256:{}", "2".repeat(64)).into(),
        tool_id: "browser_evaluate".into(),
        raw_name: "browser_evaluate".into(),
        model_name: "browser_evaluate".into(),
        upstream_schema_digest: format!("sha256:{}", "4".repeat(64)).into(),
        host_overlay_digest: format!("sha256:{}", "5".repeat(64)).into(),
        host_input_schema_digest: format!("sha256:{}", "6".repeat(64)).into(),
    };
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "http://127.0.0.1:9/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    let run_context = mycopilot_core::AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: mycopilot_core::AgentPermissions::default(),
        collaboration_identity: None,
    };
    input.context = Some(run_context.clone());
    freeze_test_pending_provider_configuration(storage, &mut input);
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        storage,
        run_id,
        Some(&action_id),
        &call,
        provenance.clone(),
    );
    checkpoint.run_context = Some(run_context);
    input.resume_checkpoint = Some(checkpoint);
    seed_durable_pending_owner(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        provenance,
        now_ms,
    );
    assert!(service
        .store_pending_action(run_id, conversation_id, assistant_message_id, action, input,)
        .unwrap());
    (action_id, call)
}
