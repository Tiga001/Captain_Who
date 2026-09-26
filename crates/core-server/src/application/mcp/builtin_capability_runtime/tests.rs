use super::*;
use base64::Engine;
use mycopilot_core::{
    builtin_capability_activation_result, BrowserDestinationIdentity, BrowserResolvedAddressClass,
    BrowserRiskKind, BrowserRiskTrigger, BuiltinMcpToolResourceSummary, BuiltinMcpToolRiskKind,
    CapabilityActivationState,
};
use mycopilot_mcp_client::{McpContentBlock, McpEmbeddedResource, McpResourceLink, McpToolResult};
use mycopilot_protocol_rs::{
    ManagedPlaywrightCommand, ManagedPlaywrightCommandNotification,
    ManagedPlaywrightCompletionInput, MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Barrier;
use std::time::Duration;

struct Harness {
    _directory: tempfile::TempDir,
    _storage: mycopilot_core::storage::service::StorageService,
    policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
    provider: Arc<HostBuiltinCapabilityProvider>,
    runtime: BuiltinCapabilityRuntime,
    now: Arc<AtomicU64>,
}

fn harness() -> Harness {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("storage.sqlite");
    let storage = mycopilot_core::storage::service::StorageService::open(&path).unwrap();
    let policies = Arc::new(SqliteBuiltinCapabilityPolicyStore::open(&path).unwrap());
    let now = Arc::new(AtomicU64::new(unix_timestamp()));
    let clock = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let provider = Arc::new(
        HostBuiltinCapabilityProvider::with_clock(Arc::clone(&policies), clock, None).unwrap(),
    );
    let runtime = BuiltinCapabilityRuntime::new(provider.clone()).unwrap();
    Harness {
        _directory: directory,
        _storage: storage,
        policies,
        provider,
        runtime,
        now,
    }
}

fn approved(
    harness: &Harness,
    action_id: Uuid,
    activation_id: Uuid,
) -> AgentBuiltinCapabilityActivationApproval {
    let manifest = &harness.runtime.manifests()[0];
    let now = harness.now.load(Ordering::SeqCst);
    AgentBuiltinCapabilityActivationApproval {
        action_id: action_id.to_string(),
        activation_id: activation_id.to_string(),
        run_id: "run-1".to_string(),
        call_id: "call-1".to_string(),
        capability_id: BROWSER_AUTOMATION_CAPABILITY_ID.to_string(),
        display_name: manifest.descriptor.display_name.clone(),
        reason: "Use the managed browser".to_string(),
        manifest_digest: manifest.manifest_digest.clone(),
        policy_revision: 1,
        created_at: now,
        expires_at: now + BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
        approval_status: AgentApprovalStatus::Approved,
    }
}

fn activate_browser(harness: &Harness) -> CapabilityGrant {
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    harness
        .runtime
        .approve_activation(&approved(harness, Uuid::new_v4(), Uuid::new_v4()))
        .unwrap()
}

fn sensitive_invocation(
    harness: &Harness,
    grant: &CapabilityGrant,
    tool_id: &str,
    call_id: &str,
    arguments: Value,
) -> BuiltinCapabilityInvocation {
    let manifest = &harness.runtime.manifests()[0];
    let descriptor = manifest
        .tools
        .iter()
        .find(|tool| tool.tool_id.as_str() == tool_id)
        .unwrap();
    BuiltinCapabilityInvocation {
        run_id: grant.run_id.clone(),
        capability_id: grant.capability_id.clone(),
        managed_mcp_id: manifest.managed_mcp_id.clone(),
        package_name: manifest.provider_contract.package_name.clone(),
        package_version: manifest.provider_contract.package_version.clone(),
        upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
        policy_digest: manifest.provider_contract.policy_digest.clone(),
        activation_id: grant.activation_id.clone(),
        manifest_digest: manifest.manifest_digest.clone(),
        policy_revision: grant.policy_revision,
        tool_id: descriptor.tool_id.as_str().to_string(),
        raw_name: descriptor.raw_name.as_str().to_string(),
        model_name: descriptor.model_name.clone(),
        upstream_schema_digest: descriptor.upstream_schema_digest.clone(),
        host_overlay_digest: descriptor.host_overlay_digest.clone(),
        host_input_schema_digest: descriptor.schema_digest.clone(),
        call_id: call_id.to_string(),
        conversation_id: None,
        arguments,
        builtin_tool_grant: None,
    }
}

fn prepare_sensitive(
    harness: &Harness,
    grant: &CapabilityGrant,
    tool_id: &str,
    call_id: &str,
    risks: Vec<BuiltinMcpToolRiskKind>,
) -> AgentResult<AgentBuiltinMcpToolApproval> {
    let profile_scoped = tool_id == "browser_set_storage_state";
    let origin = (!profile_scoped).then(|| "https://mail.example.test".to_string());
    let arguments = match tool_id {
        "browser_set_storage_state" => json!({
            "call_reason": "Import the exact reviewed storage state.",
            "filename": "browser-file:123e4567-e89b-42d3-a456-426614174000",
        }),
        "browser_network_request" => json!({
            "call_reason": "Read the exact reviewed network request.",
            "index": 0,
        }),
        _ => json!({
            "call_reason": "Perform the exact reviewed sensitive browser operation.",
            "function": "() => document.title",
        }),
    };
    let invocation = sensitive_invocation(harness, grant, tool_id, call_id, arguments);
    let file_basenames = if profile_scoped {
        vec!["storage-state.json".to_string()]
    } else {
        Vec::new()
    };
    let resource_summary = BuiltinMcpToolResourceSummary {
        scope: match tool_id {
            "browser_set_storage_state" => "storage_state_import",
            "browser_drop"
            | "browser_evaluate"
            | "browser_file_upload"
            | "browser_network_request" => "managed_surface",
            _ => "sensitive_browser_operation",
        }
        .to_string(),
        display_name: if profile_scoped {
            "Managed browser profile storage".to_string()
        } else {
            "Current managed browser surface".to_string()
        },
        file_basenames,
        origin: origin.clone(),
    };
    let request = sensitive_approval_request(
        harness,
        invocation,
        origin,
        "Perform the exact reviewed sensitive browser operation.",
        "sensitive_browser_operation",
        resource_summary,
        risks,
    )?;
    harness.provider.prepare_builtin_mcp_tool_approval(request)
}

fn sensitive_approval_request(
    harness: &Harness,
    invocation: BuiltinCapabilityInvocation,
    origin: Option<String>,
    call_reason: &str,
    operation_category: &str,
    resource_summary: BuiltinMcpToolResourceSummary,
    risks: Vec<BuiltinMcpToolRiskKind>,
) -> AgentResult<BuiltinMcpToolApprovalRequest> {
    let capability_grant = harness
        .runtime
        .live_grant(&invocation.run_id, &invocation.capability_id)?
        .ok_or_else(|| AgentError::new("test capability grant missing"))?;
    let manifest = &harness.runtime.manifests()[0];
    let descriptor = manifest
        .tools
        .iter()
        .find(|descriptor| descriptor.tool_id.as_str() == invocation.tool_id)
        .ok_or_else(|| AgentError::new("test sensitive descriptor missing"))?;
    let arguments_digest =
        mycopilot_core::builtin_mcp_tool_arguments_digest(&invocation.arguments)?;
    let binding_entropy = Uuid::new_v4().simple().to_string();
    let target_binding = PreparedBuiltinMcpToolTargetBinding {
        binding_id: Uuid::new_v4().to_string(),
        target_binding_digest: format!("sha256:{binding_entropy}{binding_entropy}"),
        origin: origin.clone(),
        created_at: harness.now.load(Ordering::SeqCst),
        expires_at: harness
            .now
            .load(Ordering::SeqCst)
            .saturating_add(mycopilot_core::BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS)
            .min(capability_grant.expires_at),
        file_basenames: resource_summary.file_basenames.clone(),
        file_revision_digest: (!resource_summary.file_basenames.is_empty())
            .then(|| format!("sha256:{}", "5".repeat(64))),
    };
    let resource_scope_digest = mycopilot_core::builtin_mcp_tool_resource_scope_digest_v2(
        &invocation.tool_id,
        &arguments_digest,
        origin.as_deref(),
        &risks,
        &resource_summary.scope,
        &target_binding.target_binding_digest,
    )?;
    Ok(BuiltinMcpToolApprovalRequest {
        invocation,
        capability_grant,
        capability_display_name: manifest.descriptor.display_name.clone(),
        tool_display_name: descriptor.model_name.clone(),
        call_reason: call_reason.to_string(),
        operation_category: operation_category.to_string(),
        resource_summary,
        resource_scope_digest,
        origin,
        risk_kinds: risks,
        target_binding: Some(target_binding),
    })
}

fn browser_risk_request(
    harness: &Harness,
    grant: &CapabilityGrant,
    normalized_url: &str,
    target_byte: char,
    trigger: BrowserRiskTrigger,
    risk_kinds: Vec<BrowserRiskKind>,
) -> BrowserRiskAuthorizationRequest {
    BrowserRiskAuthorizationRequest {
        run_id: grant.run_id.clone(),
        call_id: format!("call-risk-{target_byte}"),
        trigger_tool_name: "browser_navigate".to_string(),
        capability_id: grant.capability_id.clone(),
        capability_activation_id: grant.activation_id.clone(),
        manifest_digest: grant.manifest_digest.clone(),
        policy_revision: grant.policy_revision,
        display_name: harness.runtime.manifests()[0]
            .descriptor
            .display_name
            .clone(),
        reason: "Open the reviewed destination.".to_string(),
        destination: BrowserDestinationIdentity {
            normalized_url: normalized_url.to_string(),
            origin: "http://127.0.0.1:8765".to_string(),
            scheme: "http".to_string(),
            ascii_host: "127.0.0.1".to_string(),
            effective_port: 8765,
            address_class: BrowserResolvedAddressClass::Loopback,
        },
        resolution_fingerprint: format!("hmac-sha256:{}", "b".repeat(64)),
        target_fingerprint: format!("hmac-sha256:{}", target_byte.to_string().repeat(64)),
        trigger,
        risk_kinds,
    }
}

#[test]
fn defaults_disabled_and_reviewed_manifest_does_not_start_any_harness() {
    let harness = harness();
    let manifest = &harness.runtime.manifests()[0];
    assert_eq!(
        manifest.descriptor.id.as_str(),
        BROWSER_AUTOMATION_CAPABILITY_ID
    );
    assert!(!manifest.tools.is_empty());
    assert!(
        !harness
            .runtime
            .policy(&manifest.descriptor.id)
            .unwrap()
            .user_allowed
    );
}

#[test]
fn approval_is_exact_atomic_and_one_time() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    let grant = harness.runtime.approve_activation(&approval).unwrap();
    assert_eq!(grant.run_id, "run-1");
    assert_eq!(
        harness
            .runtime
            .live_grant("run-1", &grant.capability_id)
            .unwrap(),
        Some(grant.clone())
    );
    assert!(harness.runtime.approve_activation(&approval).is_err());

    let result =
        builtin_capability_activation_result(&approval, CapabilityActivationState::Active, None);
    assert!(result.ok);
}

#[test]
fn builtin_sensitive_approval_requires_the_exact_reviewed_risk_set() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let exact = vec![
        BuiltinMcpToolRiskKind::FileRead,
        BuiltinMcpToolRiskKind::CookieWrite,
        BuiltinMcpToolRiskKind::LocalStorageWrite,
        BuiltinMcpToolRiskKind::StorageStateImport,
    ];
    assert!(prepare_sensitive(
        &harness,
        &grant,
        "browser_set_storage_state",
        "call-risk-subset",
        vec![BuiltinMcpToolRiskKind::StorageStateImport],
    )
    .is_err());
    assert!(harness
        .provider
        .lock_grants()
        .unwrap()
        .pending_builtin_tools
        .is_empty());

    let approval = prepare_sensitive(
        &harness,
        &grant,
        "browser_set_storage_state",
        "call-risk-exact",
        exact,
    )
    .unwrap();
    let mut understated = approval.clone();
    understated.approval_status = AgentApprovalStatus::Approved;
    understated.risk_kinds = vec![BuiltinMcpToolRiskKind::StorageStateImport];
    assert!(harness
        .runtime
        .approve_builtin_mcp_tool(&understated)
        .is_err());
    let mut approved = approval.clone();
    approved.approval_status = AgentApprovalStatus::Approved;
    let grant = harness
        .runtime
        .approve_builtin_mcp_tool(&approved)
        .expect("a forged subset decision must not consume the exact pending approval");
    harness
        .runtime
        .revoke_builtin_mcp_tool_grant(&grant.grant_id, &grant.approval_id)
        .unwrap();
}

#[test]
fn concurrent_exact_sensitive_prepare_creates_one_pending_authority() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let invocation = sensitive_invocation(
        &harness,
        &grant,
        "browser_evaluate",
        "call-concurrent-prepare",
        json!({
            "function": "() => document.title",
            "call_reason": "Read the reviewed page title.",
        }),
    );
    let requests = (0..2)
        .map(|_| {
            sensitive_approval_request(
                &harness,
                invocation.clone(),
                Some("https://mail.example.test".to_string()),
                "Read the reviewed page title.",
                "page_script_execution",
                BuiltinMcpToolResourceSummary {
                    scope: "managed_surface".to_string(),
                    display_name: "Current managed browser surface".to_string(),
                    file_basenames: Vec::new(),
                    origin: Some("https://mail.example.test".to_string()),
                },
                vec![BuiltinMcpToolRiskKind::PageScriptExecution],
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(3));
    let mut joins = Vec::new();
    for request in requests {
        let provider = Arc::clone(&harness.provider);
        let barrier = Arc::clone(&barrier);
        joins.push(std::thread::spawn(move || {
            barrier.wait();
            provider.prepare_builtin_mcp_tool_approval(request)
        }));
    }
    barrier.wait();
    let results = joins
        .into_iter()
        .map(|join| join.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let duplicate = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one concurrent duplicate must fail closed");
    assert_eq!(
        duplicate.code(),
        Some("builtin_mcp_tool.approval_already_pending")
    );
    assert_eq!(
        harness
            .provider
            .lock_grants()
            .unwrap()
            .pending_builtin_tools
            .len(),
        1
    );
    let approval = results.into_iter().find_map(Result::ok).unwrap();
    harness
        .runtime
        .dismiss_builtin_mcp_tool_approval(&approval)
        .unwrap();
}

#[test]
fn network_request_index_is_approved_within_the_frozen_managed_surface() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let approval = prepare_sensitive(
        &harness,
        &grant,
        "browser_network_request",
        "call-network-ledger-index",
        vec![BuiltinMcpToolRiskKind::NetworkSensitiveRead],
    )
    .unwrap();
    assert_eq!(approval.resource_summary.scope, "managed_surface");
    assert_eq!(
        harness
            .provider
            .lock_grants()
            .unwrap()
            .pending_builtin_tools
            .len(),
        1
    );
    harness
        .runtime
        .dismiss_builtin_mcp_tool_approval(&approval)
        .unwrap();
}

#[test]
fn approve_reject_race_has_exactly_one_terminal_authority() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let approval = prepare_sensitive(
        &harness,
        &grant,
        "browser_evaluate",
        "call-approve-reject-race",
        vec![BuiltinMcpToolRiskKind::PageScriptExecution],
    )
    .unwrap();
    let mut approved = approval.clone();
    approved.approval_status = AgentApprovalStatus::Approved;
    let barrier = Arc::new(Barrier::new(3));
    let approve = {
        let runtime = harness.runtime.clone();
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            runtime.approve_builtin_mcp_tool(&approved)
        })
    };
    let reject = {
        let runtime = harness.runtime.clone();
        let approval = approval.clone();
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            runtime.reject_builtin_mcp_tool_approval(&approval)
        })
    };
    barrier.wait();
    let approved_result = approve.join().unwrap();
    let rejected_result = reject.join().unwrap();
    assert_eq!(
        usize::from(approved_result.is_ok()) + usize::from(rejected_result.is_ok()),
        1
    );
    let state = harness.provider.lock_grants().unwrap();
    assert!(state.pending_builtin_tools.is_empty());
    assert_eq!(
        state.approved_builtin_tools.len()
            + usize::from(!state.denied_builtin_tool_scopes.is_empty()),
        1
    );
}

#[test]
fn revoke_run_racing_sensitive_approve_leaves_no_live_authority() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let approval = prepare_sensitive(
        &harness,
        &grant,
        "browser_evaluate",
        "call-revoke-approve-race",
        vec![BuiltinMcpToolRiskKind::PageScriptExecution],
    )
    .unwrap();
    let mut approved = approval.clone();
    approved.approval_status = AgentApprovalStatus::Approved;
    let barrier = Arc::new(Barrier::new(3));
    let approve = {
        let runtime = harness.runtime.clone();
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            runtime.approve_builtin_mcp_tool(&approved)
        })
    };
    let revoke = {
        let runtime = harness.runtime.clone();
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            runtime.revoke_run_grants("run-1")
        })
    };
    barrier.wait();
    let _ = approve.join().unwrap();
    revoke.join().unwrap().unwrap();
    assert!(harness
        .runtime
        .live_grant("run-1", &grant.capability_id)
        .unwrap()
        .is_none());
    let state = harness.provider.lock_grants().unwrap();
    assert!(state.pending_builtin_tools.is_empty());
    assert!(state.approved_builtin_tools.is_empty());
}

#[tokio::test]
async fn tampered_invoke_does_not_consume_the_valid_single_use_grant() {
    let harness = harness();
    let capability_grant = activate_browser(&harness);
    let approval = prepare_sensitive(
        &harness,
        &capability_grant,
        "browser_evaluate",
        "call-tampered-invoke",
        vec![BuiltinMcpToolRiskKind::PageScriptExecution],
    )
    .unwrap();
    let mut approved = approval.clone();
    approved.approval_status = AgentApprovalStatus::Approved;
    let grant = harness.runtime.approve_builtin_mcp_tool(&approved).unwrap();
    let mut tampered = grant.clone();
    tampered.arguments_digest = format!("sha256:{}", "0".repeat(64));
    assert!(
        harness
            .runtime
            .invoke_approved_builtin_mcp_tool(
                approved.clone(),
                tampered,
                AgentCancellationToken::new(),
            )
            .await
            .is_err()
    );
    assert_eq!(
        harness
            .provider
            .lock_grants()
            .unwrap()
            .approved_builtin_tools
            .len(),
        1,
        "a malformed queue attempt must leave the valid authority intact"
    );

    let valid_attempt = harness
        .runtime
        .invoke_approved_builtin_mcp_tool(
            approved.clone(),
            grant.clone(),
            AgentCancellationToken::new(),
        )
        .await;
    assert!(
        valid_attempt.is_err(),
        "the test Host has no managed runtime"
    );
    assert!(harness
        .provider
        .lock_grants()
        .unwrap()
        .approved_builtin_tools
        .is_empty());
    assert!(harness
        .runtime
        .invoke_approved_builtin_mcp_tool(approved, grant, AgentCancellationToken::new(),)
        .await
        .is_err());
}

#[tokio::test]
async fn cancellation_before_sensitive_grant_consumption_is_definite_and_single_use() {
    let harness = harness();
    let capability_grant = activate_browser(&harness);
    let approval = prepare_sensitive(
        &harness,
        &capability_grant,
        "browser_evaluate",
        "call-cancelled-before-consumption",
        vec![BuiltinMcpToolRiskKind::PageScriptExecution],
    )
    .unwrap();
    let mut approved = approval.clone();
    approved.approval_status = AgentApprovalStatus::Approved;
    let grant = harness.runtime.approve_builtin_mcp_tool(&approved).unwrap();
    let cancellation = AgentCancellationToken::new();
    cancellation.cancel();

    let error = harness
        .runtime
        .invoke_approved_builtin_mcp_tool(approved.clone(), grant.clone(), cancellation)
        .await
        .unwrap_err();
    assert!(error.is_cancelled());
    assert_eq!(error.code(), Some("mcp.tool_cancelled_before_dispatch"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("dispatchCertainty"))
            .and_then(Value::as_str),
        Some("definitely_not_dispatched")
    );
    assert!(harness
        .provider
        .lock_grants()
        .unwrap()
        .approved_builtin_tools
        .is_empty());
    assert!(harness
        .runtime
        .invoke_approved_builtin_mcp_tool(approved, grant, AgentCancellationToken::new())
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_between_dispatch_cas_and_grant_consumption_stays_definite() {
    let harness = harness();
    let capability_grant = activate_browser(&harness);
    let approval = prepare_sensitive(
        &harness,
        &capability_grant,
        "browser_evaluate",
        "call-cancelled-at-consumption-barrier",
        vec![BuiltinMcpToolRiskKind::PageScriptExecution],
    )
    .unwrap();
    let mut approved = approval.clone();
    approved.approval_status = AgentApprovalStatus::Approved;
    let grant = harness.runtime.approve_builtin_mcp_tool(&approved).unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let hook = {
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        Arc::new(move || {
            entered.wait();
            release.wait();
        }) as Arc<dyn Fn() + Send + Sync>
    };
    *harness
        .provider
        .sensitive_before_grant_consume_hook
        .lock()
        .unwrap() = Some(hook);
    let cancellation = AgentCancellationToken::new();
    let invoke_cancellation = cancellation.clone();
    let runtime = harness.runtime.clone();
    let invoke = tokio::spawn(async move {
        runtime
            .invoke_approved_builtin_mcp_tool(approved, grant, invoke_cancellation)
            .await
    });
    entered.wait();
    cancellation.cancel();
    release.wait();
    let error = invoke.await.unwrap().unwrap_err();
    assert!(error.is_cancelled());
    assert_eq!(error.code(), Some("mcp.tool_cancelled_before_dispatch"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("dispatchCertainty"))
            .and_then(Value::as_str),
        Some("definitely_not_dispatched")
    );
    assert!(harness
        .provider
        .lock_grants()
        .unwrap()
        .approved_builtin_tools
        .is_empty());
}

#[test]
fn builtin_sensitive_rejection_blocks_exact_repeat_and_cleanup_is_idempotent() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let risks = vec![BuiltinMcpToolRiskKind::PageScriptExecution];
    let approval = prepare_sensitive(
        &harness,
        &grant,
        "browser_evaluate",
        "call-sensitive-first",
        risks.clone(),
    )
    .unwrap();
    harness
        .runtime
        .reject_builtin_mcp_tool_approval(&approval)
        .unwrap();
    let repeated = prepare_sensitive(
        &harness,
        &grant,
        "browser_evaluate",
        "call-sensitive-repeat",
        risks,
    )
    .unwrap_err();
    assert_eq!(
        repeated.code(),
        Some("builtin_mcp_tool.previously_rejected")
    );
    harness
        .runtime
        .dismiss_builtin_mcp_tool_approval(&approval)
        .unwrap();
    harness
        .runtime
        .dismiss_builtin_mcp_tool_approval(&approval)
        .unwrap();
    let state = harness.provider.lock_grants().unwrap();
    assert!(state.pending_builtin_tools.is_empty());
    assert!(state.approved_builtin_tools.is_empty());
}

#[tokio::test]
async fn main_frozen_origin_replaces_the_model_claim_in_approval_and_scope() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let managed_runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
    harness
        .provider
        .attach_managed_runtime(Arc::clone(&managed_runtime))
        .unwrap();
    let (outbound, mut commands) = tokio::sync::mpsc::unbounded_channel();
    managed_runtime.bridge().attach_outbound(outbound).unwrap();

    let claimed_origin = "https://model-claim.example.test";
    let frozen_origin = "https://mail.example.test";
    let invocation = sensitive_invocation(
        &harness,
        &grant,
        "browser_evaluate",
        "call-main-origin-authority",
        json!({
            "function": "() => document.title",
            "call_reason": "Read the reviewed fixture title."
        }),
    );
    let expected_arguments_digest =
        mycopilot_core::builtin_mcp_tool_arguments_digest(&invocation.arguments).unwrap();
    let runtime = harness.runtime.clone();
    let proposal = tokio::spawn(async move {
        runtime
            .prepare_builtin_mcp_tool_approval_async(
                invocation,
                "The model requests running a reviewed script in the managed page.".to_string(),
                "page_script_execution".to_string(),
                BuiltinMcpToolResourceSummary {
                    scope: "managed_surface".to_string(),
                    display_name: "Current managed browser surface".to_string(),
                    file_basenames: Vec::new(),
                    origin: Some(claimed_origin.to_string()),
                },
                None,
                vec![BuiltinMcpToolRiskKind::PageScriptExecution],
                AgentCancellationToken::new(),
            )
            .await
    });

    let prepare: ManagedPlaywrightCommandNotification = serde_json::from_value(
        commands.recv().await.expect("prepare command missing")["params"].clone(),
    )
    .unwrap();
    let ManagedPlaywrightCommand::PrepareSensitiveTool { input } = &prepare.command else {
        panic!("expected proposal-time target prepare command");
    };
    let binding_id = Uuid::new_v4().to_string();
    let target_binding_digest = format!("sha256:{}", "8".repeat(64));
    assert!(managed_runtime
        .bridge()
        .complete(ManagedPlaywrightCompletionInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: prepare.request_id,
            outcome: ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared {
                binding_id,
                target_binding_digest,
                origin: Some(frozen_origin.to_string()),
                created_at_ms: input.created_at_ms,
                expires_at_ms: input.expires_at_ms,
                file_basenames: Vec::new(),
                file_revision_digest: None,
            },
        })
        .unwrap());

    let approval = proposal.await.unwrap().unwrap();
    assert_eq!(
        approval.identity.arguments_digest,
        expected_arguments_digest
    );
    assert_eq!(approval.identity.origin.as_deref(), Some(frozen_origin));
    assert_eq!(approval.resource_summary.scope, "managed_surface");
    assert_eq!(
        approval.resource_summary.origin.as_deref(),
        Some(frozen_origin)
    );
    assert_ne!(
        approval.resource_summary.origin.as_deref(),
        Some(claimed_origin)
    );
    harness
        .runtime
        .dismiss_builtin_mcp_tool_approval(&approval)
        .unwrap();
}

#[tokio::test]
async fn cancelled_target_prepare_drains_late_success_and_releases_exact_binding() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let managed_runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
    harness
        .provider
        .attach_managed_runtime(Arc::clone(&managed_runtime))
        .unwrap();
    let (outbound, mut commands) = tokio::sync::mpsc::unbounded_channel();
    managed_runtime.bridge().attach_outbound(outbound).unwrap();

    let invocation = sensitive_invocation(
        &harness,
        &grant,
        "browser_evaluate",
        "call-cancelled-prepare",
        json!({
            "function": "() => document.title",
            "call_reason": "Read the reviewed fixture title."
        }),
    );
    let cancellation = AgentCancellationToken::new();
    let invoke_cancellation = cancellation.clone();
    let runtime = harness.runtime.clone();
    let proposal = tokio::spawn(async move {
        runtime
            .prepare_builtin_mcp_tool_approval_async(
                invocation,
                "The model requests running a reviewed script in the managed page.".to_string(),
                "page_script_execution".to_string(),
                BuiltinMcpToolResourceSummary {
                    scope: "managed_surface".to_string(),
                    display_name: "Current managed browser surface".to_string(),
                    file_basenames: Vec::new(),
                    origin: Some("https://mail.example.test".to_string()),
                },
                None,
                vec![BuiltinMcpToolRiskKind::PageScriptExecution],
                invoke_cancellation,
            )
            .await
    });

    let prepare: ManagedPlaywrightCommandNotification = serde_json::from_value(
        commands.recv().await.expect("prepare command missing")["params"].clone(),
    )
    .unwrap();
    let ManagedPlaywrightCommand::PrepareSensitiveTool { input } = prepare.command else {
        panic!("expected proposal-time target prepare command");
    };
    cancellation.cancel();
    // AgentCancellationToken currently polls at a bounded 50 ms cadence. Keep Main pending
    // until the cancellation branch is waiting to drain this exact completion.
    tokio::time::sleep(Duration::from_millis(75)).await;
    let binding_id = Uuid::new_v4().to_string();
    assert!(managed_runtime
        .bridge()
        .complete(ManagedPlaywrightCompletionInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: prepare.request_id,
            outcome: ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared {
                binding_id: binding_id.clone(),
                target_binding_digest: format!("sha256:{}", "8".repeat(64)),
                origin: Some("https://mail.example.test".to_string()),
                created_at_ms: input.created_at_ms,
                expires_at_ms: input.expires_at_ms,
                file_basenames: Vec::new(),
                file_revision_digest: None,
            },
        })
        .unwrap());

    let release: ManagedPlaywrightCommandNotification = serde_json::from_value(
        tokio::time::timeout(Duration::from_secs(1), commands.recv())
            .await
            .expect("release command timed out")
            .expect("release command missing")["params"]
            .clone(),
    )
    .unwrap();
    assert!(matches!(
        release.command,
        ManagedPlaywrightCommand::ReleaseSensitiveToolBinding {
            binding_id: released_binding,
            reason: ManagedPlaywrightSensitiveBindingReleaseReason::Cancelled,
            ..
        } if released_binding == binding_id
    ));
    let error = proposal.await.unwrap().unwrap_err();
    assert_eq!(error.code(), Some("mcp.tool_cancelled_before_dispatch"));
    assert!(harness
        .provider
        .lock_grants()
        .unwrap()
        .pending_builtin_tools
        .is_empty());
}

#[test]
fn pending_sensitive_action_serialization_contains_only_the_safe_projection() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let secret = "BUILTIN_PENDING_SECRET_CANARY";
    let invocation = sensitive_invocation(
        &harness,
        &grant,
        "browser_evaluate",
        "call-private-pending",
        json!({
            "function": format!("() => document.cookie + '{secret}'"),
            "password": secret,
            "authorization": format!("Bearer {secret}"),
            "storageValue": secret,
            "path": format!("browser-file:{secret}"),
            "call_reason": "Read the reviewed page state.",
        }),
    );
    let request = sensitive_approval_request(
        &harness,
        invocation,
        Some("https://mail.example.test".to_string()),
        "Read the reviewed page state.",
        "page_script_execution",
        BuiltinMcpToolResourceSummary {
            scope: "managed_surface".to_string(),
            display_name: "Current managed browser surface".to_string(),
            file_basenames: Vec::new(),
            origin: Some("https://mail.example.test".to_string()),
        },
        vec![BuiltinMcpToolRiskKind::PageScriptExecution],
    )
    .unwrap();
    let approval = harness
        .provider
        .prepare_builtin_mcp_tool_approval(request)
        .unwrap();
    let action = mycopilot_core::AgentProposedAction::BuiltinMcpToolApproval {
        approval: Box::new(approval.clone()),
    };
    let serialized_action = serde_json::to_string(&action).unwrap();
    assert!(!serialized_action.contains(secret));
    assert!(!serialized_action.contains("document.cookie"));
    assert!(!serialized_action.contains("browser-file:"));
    assert!(!serialized_action.contains("authorization"));
    let state = harness.provider.lock_grants().unwrap();
    let sealed = state
        .pending_builtin_tools
        .get(&approval.identity.approval_id)
        .unwrap();
    assert!(serde_json::to_string(&sealed.invocation.arguments)
        .unwrap()
        .contains(secret));
}

#[test]
fn browser_risk_grants_reuse_only_the_frozen_safe_scope() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let activation = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    let capability_grant = harness.runtime.approve_activation(&activation).unwrap();
    let request = browser_risk_request(
        &harness,
        &capability_grant,
        "http://127.0.0.1:8765/a",
        'c',
        BrowserRiskTrigger::ToolArgument,
        vec![BrowserRiskKind::InsecureHttp, BrowserRiskKind::Loopback],
    );
    let mut approval = harness
        .runtime
        .prepare_browser_risk_approval(&request)
        .unwrap();
    approval.approval_status = AgentApprovalStatus::Approved;
    let risk_grant = harness.runtime.approve_browser_risk(&approval).unwrap();
    assert!(harness.runtime.approve_browser_risk(&approval).is_err());
    assert_eq!(
        harness.runtime.live_browser_risk_grant(&request).unwrap(),
        Some(risk_grant.clone())
    );

    // A stable resolver answer and risk set may reuse an origin grant across paths.
    let same_origin = browser_risk_request(
        &harness,
        &capability_grant,
        "http://127.0.0.1:8765/b",
        'd',
        BrowserRiskTrigger::Redirect,
        vec![BrowserRiskKind::InsecureHttp, BrowserRiskKind::Loopback],
    );
    assert!(harness
        .runtime
        .live_browser_risk_grant(&same_origin)
        .unwrap()
        .is_some());

    let mut resolver_drift = same_origin.clone();
    resolver_drift.resolution_fingerprint = format!("hmac-sha256:{}", "e".repeat(64));
    assert!(harness
        .runtime
        .live_browser_risk_grant(&resolver_drift)
        .unwrap()
        .is_none());

    let mut risk_drift = same_origin;
    risk_drift.risk_kinds.push(BrowserRiskKind::NonStandardPort);
    risk_drift.risk_kinds.sort_unstable();
    assert!(harness
        .runtime
        .live_browser_risk_grant(&risk_drift)
        .unwrap()
        .is_none());

    harness
        .now
        .store(risk_grant.expires_at.saturating_add(1), Ordering::SeqCst);
    // Browser risk authority never outlives its task-level capability grant. Once that
    // parent grant expires, lookup fails closed instead of treating the request as a fresh,
    // independently approvable browser destination.
    assert!(harness.runtime.live_browser_risk_grant(&request).is_err());
}

#[test]
fn sensitive_browser_actions_bind_exact_target_trigger_and_tool() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let activation = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    let capability_grant = harness.runtime.approve_activation(&activation).unwrap();
    let request = browser_risk_request(
        &harness,
        &capability_grant,
        "http://127.0.0.1:8765/upload",
        'c',
        BrowserRiskTrigger::Upload,
        vec![
            BrowserRiskKind::InsecureHttp,
            BrowserRiskKind::Loopback,
            BrowserRiskKind::FileUpload,
        ],
    );
    let mut approval = harness
        .runtime
        .prepare_browser_risk_approval(&request)
        .unwrap();
    approval.approval_status = AgentApprovalStatus::Approved;
    harness.runtime.approve_browser_risk(&approval).unwrap();

    let mut target_drift = request.clone();
    target_drift.target_fingerprint = format!("hmac-sha256:{}", "d".repeat(64));
    assert!(harness
        .runtime
        .live_browser_risk_grant(&target_drift)
        .unwrap()
        .is_none());
    let mut trigger_drift = request.clone();
    trigger_drift.trigger = BrowserRiskTrigger::NewWindow;
    assert!(harness
        .runtime
        .live_browser_risk_grant(&trigger_drift)
        .unwrap()
        .is_none());
    let mut tool_drift = request;
    tool_drift.trigger_tool_name = "browser_click".to_string();
    assert!(harness
        .runtime
        .live_browser_risk_grant(&tool_drift)
        .unwrap()
        .is_none());
}

#[test]
fn terminal_run_retires_capability_risk_and_pending_authority() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let activation = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    let capability_grant = harness.runtime.approve_activation(&activation).unwrap();
    let request = browser_risk_request(
        &harness,
        &capability_grant,
        "http://127.0.0.1:8765/a",
        'c',
        BrowserRiskTrigger::ToolArgument,
        vec![BrowserRiskKind::InsecureHttp, BrowserRiskKind::Loopback],
    );
    let approval = harness
        .runtime
        .prepare_browser_risk_approval(&request)
        .unwrap();
    assert_eq!(
        harness
            .provider
            .lock_grants()
            .unwrap()
            .pending_browser_risks
            .len(),
        1
    );

    harness.runtime.revoke_run_grants("run-1").unwrap();
    let state = harness.provider.lock_grants().unwrap();
    assert!(state.grants.is_empty());
    assert!(state.browser_risk_grants.is_empty());
    assert!(state.pending_browser_risks.is_empty());
    drop(state);
    assert!(harness.runtime.approve_browser_risk(&approval).is_err());
}

#[test]
fn exact_activation_rollback_removes_grant_and_restores_one_shot_ids() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    let grant = harness.runtime.approve_activation(&approval).unwrap();
    assert!(harness
        .runtime
        .live_grant("run-1", &grant.capability_id)
        .unwrap()
        .is_some());

    harness
        .runtime
        .revoke_activation(&grant.activation_id, &approval.action_id)
        .unwrap();
    assert!(harness
        .runtime
        .live_grant("run-1", &grant.capability_id)
        .unwrap()
        .is_none());
    assert!(harness.provider.lock_grants().unwrap().grants.is_empty());

    // A durable settlement failure may retry the exact pending approval; rollback must not
    // leave either one-shot identity consumed.
    assert!(harness.runtime.approve_activation(&approval).is_ok());
}

#[test]
fn revoking_a_settled_grant_does_not_reopen_its_activation_identity() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    let grant = harness.runtime.approve_activation(&approval).unwrap();
    {
        let state = harness.provider.lock_grants().unwrap();
        assert_eq!(
            state.consumed_action_ids.get(&approval.action_id),
            Some(&grant.expires_at)
        );
        assert_eq!(
            state.consumed_activation_ids.get(&approval.activation_id),
            Some(&grant.expires_at)
        );
    }

    harness.runtime.revoke_grants(&grant.capability_id).unwrap();
    assert!(harness.runtime.approve_activation(&approval).is_err());
}

#[test]
fn durable_disable_committed_during_approval_rolls_back_grant_before_start() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let hook = {
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        Arc::new(move || {
            entered.wait();
            release.wait();
        }) as Arc<dyn Fn() + Send + Sync>
    };
    *harness.provider.approval_before_start_hook.lock().unwrap() = Some(hook);

    let provider = Arc::clone(&harness.provider);
    let approval_for_thread = approval.clone();
    let settlement = std::thread::spawn(move || provider.approve_activation(&approval_for_thread));
    entered.wait();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 1, false)
        .unwrap();
    release.wait();

    assert!(settlement.join().unwrap().is_err());
    let state = harness.provider.lock_grants().unwrap();
    assert!(state.grants.is_empty());
    assert!(state.consumed_action_ids.is_empty());
    assert!(state.consumed_activation_ids.is_empty());
}

#[test]
fn policy_disable_and_expiry_make_process_grant_immediately_unusable() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    let grant = harness.runtime.approve_activation(&approval).unwrap();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 1, false)
        .unwrap();
    harness.runtime.revoke_grants(&grant.capability_id).unwrap();
    assert!(harness.provider.lock_grants().unwrap().grants.is_empty());
    assert!(harness
        .runtime
        .live_grant("run-1", &grant.capability_id)
        .unwrap()
        .is_none());

    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 2, true)
        .unwrap();
    assert!(harness
        .runtime
        .live_grant("run-1", &grant.capability_id)
        .unwrap()
        .is_none());
    harness
        .now
        .store(grant.expires_at.saturating_add(1), Ordering::SeqCst);
    assert!(harness
        .provider
        .grant("run-1", &grant.capability_id)
        .unwrap()
        .is_none());
    let state = harness.provider.lock_grants().unwrap();
    assert!(state.grants.is_empty());
    assert!(state.consumed_action_ids.is_empty());
    assert!(state.consumed_activation_ids.is_empty());
}

#[tokio::test]
async fn host_rejects_frozen_late_invocation_after_disable_and_reenable() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let invocation = sensitive_invocation(
        &harness,
        &grant,
        "browser_snapshot",
        "late-browser-call",
        json!({"call_reason": "Inspect the current page"}),
    );
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 1, false)
        .unwrap();
    // Exercise the Host boundary directly, bypassing Core's already-rechecked tool gate.
    // Keep the old in-memory grant to prove durable policy invalidates it independently.
    let error = harness
        .provider
        .invoke_authorized(
            invocation.clone(),
            grant.clone(),
            AgentCancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("grant 已失效"));
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 2, true)
        .unwrap();
    let error = harness
        .provider
        .invoke_authorized(invocation, grant, AgentCancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("grant 已失效"));
}

#[test]
fn restarted_host_recovers_enabled_policy_without_task_grants() {
    let harness = harness();
    let grant = activate_browser(&harness);
    let reopened = Arc::new(
        SqliteBuiltinCapabilityPolicyStore::open(harness._directory.path().join("storage.sqlite"))
            .unwrap(),
    );
    let (restarted, provider) =
        HostBuiltinCapabilityProvider::runtime_and_provider(reopened, None).unwrap();
    assert!(restarted.policy(&grant.capability_id).unwrap().user_allowed);
    assert_eq!(restarted.policy(&grant.capability_id).unwrap().revision, 1);
    assert!(restarted
        .live_grant(&grant.run_id, &grant.capability_id)
        .unwrap()
        .is_none());
    assert!(provider.lock_grants().unwrap().grants.is_empty());
    assert!(provider.managed_runtime().unwrap().is_none());
}

#[test]
fn approval_rejects_manifest_policy_expiry_and_id_drift() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let base = approved(&harness, Uuid::new_v4(), Uuid::new_v4());

    let mut drifted = base.clone();
    drifted.manifest_digest = "sha256:deadbeef".to_string();
    assert!(harness.runtime.approve_activation(&drifted).is_err());

    let mut expired = base.clone();
    expired.expires_at = expired.created_at;
    assert!(harness.runtime.approve_activation(&expired).is_err());

    let mut malformed = base;
    malformed.action_id = Uuid::nil().to_string();
    assert!(harness.runtime.approve_activation(&malformed).is_err());

    let policy_drift = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 1, false)
        .unwrap();
    assert!(harness.provider.approve_activation(&policy_drift).is_err());
    assert!(harness.provider.lock_grants().unwrap().grants.is_empty());
}

#[tokio::test]
async fn unreviewed_tool_invocation_fails_closed_without_starting_a_harness() {
    let harness = harness();
    harness
        .policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .unwrap();
    let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
    let grant = harness.runtime.approve_activation(&approval).unwrap();
    let manifest = harness.runtime.manifests()[0].clone();
    let invocation = BuiltinCapabilityInvocation {
        run_id: grant.run_id.clone(),
        capability_id: grant.capability_id.clone(),
        managed_mcp_id: BROWSER_AUTOMATION_MANAGED_MCP_ID.to_string(),
        package_name: manifest.provider_contract.package_name.clone(),
        package_version: manifest.provider_contract.package_version.clone(),
        upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
        policy_digest: manifest.provider_contract.policy_digest.clone(),
        activation_id: grant.activation_id.clone(),
        manifest_digest: grant.manifest_digest.clone(),
        policy_revision: grant.policy_revision,
        tool_id: "browser_unreviewed".to_string(),
        raw_name: "browser_unreviewed".to_string(),
        model_name: "browser_unreviewed".to_string(),
        upstream_schema_digest: format!("sha256:{}", "a".repeat(64)),
        host_overlay_digest: format!("sha256:{}", "b".repeat(64)),
        host_input_schema_digest: format!("sha256:{}", "c".repeat(64)),
        call_id: "call-unreviewed".to_string(),
        conversation_id: None,
        arguments: serde_json::json!({}),
        builtin_tool_grant: None,
    };
    let error = harness
        .provider
        .invoke_authorized(invocation, grant, AgentCancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("不在当前审核 manifest"));
}

#[test]
fn managed_result_projection_omits_binary_and_resource_identity() {
    let projected = project_managed_browser_result(McpToolResult {
        content: vec![
            McpContentBlock::Text {
                text: "visible text".to_string(),
            },
            McpContentBlock::Image {
                data: "private-image-canary".to_string(),
                mime_type: "image/png".to_string(),
            },
            McpContentBlock::Audio {
                data: "private-audio-canary".to_string(),
                mime_type: "audio/wav".to_string(),
            },
            McpContentBlock::EmbeddedResource {
                resource: McpEmbeddedResource::Text {
                    uri: "private-resource-uri-canary".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    text: "private-resource-body-canary".to_string(),
                },
            },
            McpContentBlock::ResourceLink {
                resource: McpResourceLink {
                    uri: "private-link-uri-canary".to_string(),
                    name: "private-link-name-canary".to_string(),
                    title: None,
                    description: None,
                    mime_type: None,
                    size: Some(42),
                },
            },
        ],
        structured_content: Some(json!({"safe": true})),
        is_error: false,
    })
    .unwrap();
    let serialized = serde_json::to_string(&projected).unwrap();
    assert!(serialized.contains("visible text"));
    assert!(serialized.contains("embedded_resource"));
    for canary in [
        "private-image-canary",
        "private-audio-canary",
        "private-resource-uri-canary",
        "private-resource-body-canary",
        "private-link-uri-canary",
        "private-link-name-canary",
    ] {
        assert!(!serialized.contains(canary), "leaked {canary}");
    }
}

#[test]
fn managed_download_progress_reaches_the_agent_model_projection() {
    let download_id = "browser-download:123e4567-e89b-42d3-a456-426614174000";
    let progress = json!({
        "downloadId": download_id,
        "displayName": "fixture-download.bin",
        "mimeType": "application/octet-stream",
        "state": "progressing",
        "receivedBytes": 65_536,
        "totalBytes": 16 * 1024 * 1024,
        "bytesPerSecond": 1_638_400,
        "startedAt": 1_000,
        "updatedAt": 1_100
    });
    let projected = project_managed_browser_result(McpToolResult {
        content: vec![McpContentBlock::Text {
            text: "Download fixture-download.bin: 65.5 KB / 16.8 MB.".to_string(),
        }],
        structured_content: Some(json!({
            "status": "download_started",
            "downloadProgress": [progress.clone()]
        })),
        is_error: false,
    })
    .unwrap();

    assert_eq!(projected["outcome"], "completed");
    assert_eq!(projected["structuredContent"]["status"], "download_started");
    assert_eq!(
        projected["structuredContent"]["downloadProgress"],
        json!([progress])
    );
    let serialized = serde_json::to_string(&projected).unwrap();
    assert!(!serialized.contains("/Users/"));
    assert!(!serialized.contains("sourceUrl"));
}

#[test]
fn managed_result_projection_keeps_only_strict_artifact_references() {
    let artifact = json!({
        "schemaVersion": 1,
        "artifactId": "browser-artifact:123e4567-e89b-42d3-a456-426614174000",
        "kind": "pdf",
        "displayName": "page.pdf",
        "mimeType": "application/pdf",
        "sizeBytes": 42,
        "createdAt": 1_000,
        "expiresAt": 2_000,
        "lifecycle": "run",
        "owner": "browser_automation",
        "preview": "none"
    });
    let projected = project_managed_browser_result(McpToolResult {
        content: vec![McpContentBlock::Text {
            text: "Created a managed PDF Artifact.".to_string(),
        }],
        structured_content: Some(json!({
            "status": "completed",
            "artifacts": [artifact.clone()],
            "privatePath": "/tmp/private-output/page.pdf",
        })),
        is_error: false,
    })
    .unwrap();
    assert_eq!(
        projected["structuredContent"],
        json!({"artifacts": [artifact]})
    );
    assert!(!serde_json::to_string(&projected)
        .unwrap()
        .contains("private-output"));

    let image = json!({
        "schemaVersion": 1,
        "artifactId": "browser-artifact:123e4567-e89b-42d3-a456-426614174000",
        "kind": "image",
        "displayName": "page.png",
        "mimeType": "image/png",
        "sizeBytes": 42,
        "createdAt": 1_000,
        "expiresAt": 2_000,
        "lifecycle": "run",
        "owner": "browser_automation",
        "preview": "image"
    });
    let read_path = format!("image-artifact://sha256/{}", "a".repeat(64));
    let with_read_path = project_managed_browser_result(McpToolResult {
        content: vec![McpContentBlock::Text {
            text: "Created managed image Artifact.".to_string(),
        }],
        structured_content: Some(json!({
            "artifacts": [image.clone()],
            "readPath": read_path,
            "hostArtifactPublishPath": "/tmp/private-output/page.png",
        })),
        is_error: false,
    })
    .unwrap();
    assert_eq!(
        with_read_path["structuredContent"],
        json!({ "artifacts": [image], "readPath": read_path })
    );
    assert!(!serde_json::to_string(&with_read_path)
        .unwrap()
        .contains("private-output"));
    assert!(!serde_json::to_string(&with_read_path)
        .unwrap()
        .contains("hostArtifactPublishPath"));

    let malformed = project_managed_browser_result(McpToolResult {
        content: vec![],
        structured_content: Some(json!({
            "artifacts": [{
                "schemaVersion": 1,
                "artifactId": "browser-artifact:123e4567-e89b-42d3-a456-426614174000",
                "kind": "pdf",
                "displayName": "page.pdf",
                "mimeType": "application/pdf",
                "sizeBytes": 42,
                "createdAt": 1_000,
                "expiresAt": 2_000,
                "lifecycle": "run",
                "owner": "browser_automation",
                "preview": "none",
                "managedPath": "/tmp/private-output/page.pdf"
            }]
        })),
        is_error: false,
    })
    .unwrap();
    assert!(malformed["structuredContent"].is_null());
    assert!(!serde_json::to_string(&malformed)
        .unwrap()
        .contains("private-output"));
}

#[test]
fn screenshot_publish_attaches_canonical_read_path_and_strips_host_path() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("storage.sqlite");
    let storage = mycopilot_core::storage::service::StorageService::open(&database_path).unwrap();
    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute(
            "INSERT INTO conversations (id, title, created_at, updated_at)
             VALUES ('conversation-1', 'test', 1, 1)",
            [],
        )
        .unwrap();
    let png = base64::engine::general_purpose::STANDARD
        .decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
        )
        .unwrap();
    let source = directory.path().join("screenshot-object");
    std::fs::write(&source, &png).unwrap();
    let artifact = json!({
        "schemaVersion": 1,
        "artifactId": "browser-artifact:123e4567-e89b-42d3-a456-426614174000",
        "kind": "image",
        "displayName": "page.png",
        "mimeType": "image/png",
        "sizeBytes": png.len(),
        "createdAt": 1_000,
        "expiresAt": 2_000,
        "lifecycle": "run",
        "owner": "browser_automation",
        "preview": "image"
    });
    let attached = attach_browser_artifact_read_path(
        McpToolResult {
            content: vec![McpContentBlock::Text {
                text: "Created managed image Artifact “page.png” (68 bytes).".to_string(),
            }],
            structured_content: Some(json!({
                "status": "completed",
                "artifacts": [artifact.clone()],
                "hostArtifactPublishPath": source.to_string_lossy(),
            })),
            is_error: false,
        },
        Some(source.to_string_lossy().into_owned()),
        Some(&storage),
        Some(mycopilot_core::storage::service::ManagedArtifactAuthority {
            conversation_id: "conversation-1",
            run_id: "run-1",
            call_id: "call-1",
        }),
    );
    let read_path = attached.structured_content.as_ref().unwrap()["readPath"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(read_path.starts_with("image-artifact://sha256/"));
    assert_eq!(read_path.len(), "image-artifact://sha256/".len() + 64);
    let attached_json = serde_json::to_string(&attached).unwrap();
    assert!(!attached_json.contains("hostArtifactPublishPath"));
    assert!(!attached_json.contains("screenshot-object"));
    assert!(attached.content.iter().any(|block| matches!(
        block,
        McpContentBlock::Text { text } if text.contains(&format!("read_image.path: {read_path}"))
    )));

    let projected = project_managed_browser_result(attached).unwrap();
    assert_eq!(
        projected["structuredContent"],
        json!({ "artifacts": [artifact], "readPath": read_path })
    );
    let projected_json = serde_json::to_string(&projected).unwrap();
    assert!(!projected_json.contains("screenshot-object"));
    assert!(!projected_json.contains("hostArtifactPublishPath"));
    assert!(!projected_json.contains("managedPath"));
}

#[test]
fn pdf_publish_attaches_durable_authorized_read_path_through_result_projections() {
    use mycopilot_core::storage::service::{ManagedArtifactAuthority, StorageService};
    use sha2::{Digest, Sha256};

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("storage.sqlite");
    let storage = StorageService::open(&database_path).unwrap();
    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute(
            "INSERT INTO conversations (id, title, created_at, updated_at)
             VALUES ('conversation-1', 'test', 1, 1), ('conversation-2', 'unrelated', 1, 1)",
            [],
        )
        .unwrap();
    let pdf = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";
    let source = directory.path().join("pdf-object");
    fs::write(&source, pdf).unwrap();
    let artifact = json!({
        "schemaVersion": 1,
        "artifactId": "browser-artifact:123e4567-e89b-42d3-a456-426614174000",
        "kind": "pdf",
        "displayName": "网页.pdf",
        "mimeType": "application/pdf",
        "sizeBytes": pdf.len(),
        "createdAt": 1_000,
        "expiresAt": 2_000,
        "lifecycle": "run",
        "owner": "browser_automation",
        "preview": "none"
    });
    let attached = attach_browser_artifact_read_path(
        McpToolResult {
            content: vec![McpContentBlock::Text {
                text: "Created managed PDF Artifact.".to_string(),
            }],
            structured_content: Some(json!({
                "status": "completed",
                "artifacts": [artifact.clone()],
                "hostArtifactPublishPath": source.to_string_lossy(),
            })),
            is_error: false,
        },
        Some(source.to_string_lossy().into_owned()),
        Some(&storage),
        Some(ManagedArtifactAuthority {
            conversation_id: "conversation-1",
            run_id: "run-1",
            call_id: "call-pdf",
        }),
    );
    let digest = format!("{:x}", Sha256::digest(pdf));
    let read_path = format!("artifact://sha256/{digest}");
    assert_eq!(
        attached.structured_content.as_ref().unwrap()["readPath"],
        read_path
    );
    assert!(attached.content.iter().any(|block| matches!(
        block,
        McpContentBlock::Text { text } if text.contains(&format!("run_command.inputs[].path: {read_path}"))
            && !text.contains("read_image.path")
    )));
    assert!(!source.with_extension("pdf").exists());
    fs::remove_file(&source).unwrap();
    drop(storage);

    // The unified object and its grant survive browser source cleanup and reopening storage.
    let storage = StorageService::open(&database_path).unwrap();
    let artifact_id = format!("sha256:{digest}");
    let stored = storage
        .read_authorized_managed_artifact(&artifact_id, "conversation-1")
        .unwrap()
        .unwrap();
    assert_eq!(stored.bytes, pdf);
    assert_eq!(stored.media_type, "application/pdf");
    assert!(storage
        .read_authorized_managed_artifact(&artifact_id, "conversation-2")
        .unwrap()
        .is_none());

    let projected = project_managed_browser_result(attached).unwrap();
    assert_eq!(
        projected["structuredContent"],
        json!({ "artifacts": [artifact.clone()], "readPath": read_path })
    );
    let result = mycopilot_core::protocol::AgentToolResult {
        exact_archive_file: None,
        call_id: "call-pdf".to_string(),
        tool: "browser_pdf_save".to_string(),
        ok: true,
        result: Some(projected.clone()),
        error: None,
    };
    let persisted = mycopilot_core::builtin_capability_tool_result_persistence_projection(&result);
    assert_eq!(persisted.result.as_ref().unwrap()["readPath"], read_path);
    assert_eq!(
        persisted.result.as_ref().unwrap()["artifacts"],
        json!([artifact])
    );
    for json in [
        serde_json::to_string(&projected).unwrap(),
        serde_json::to_string(&persisted).unwrap(),
    ] {
        assert!(!json.contains("pdf-object"));
        assert!(!json.contains("hostArtifactPublishPath"));
    }
}

#[test]
fn authoritative_managed_tool_error_is_failed_with_response_received() {
    let error = project_managed_browser_result(McpToolResult {
        content: vec![McpContentBlock::Text {
            text: "reviewed tool error".to_string(),
        }],
        structured_content: None,
        is_error: true,
    })
    .unwrap_err();
    assert_eq!(error.code(), Some("builtin.browser_automation.tool_error"));
    let details = error.details().unwrap();
    assert_eq!(details["outcome"], "tool_error");
    assert_eq!(details["isError"], true);
    assert_eq!(details["dispatchCertainty"], "response_received");
    assert_eq!(details["content"][0]["text"], "reviewed tool error");
}
