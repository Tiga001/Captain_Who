use super::*;

fn builtin_mcp_approval() -> mycopilot_core::AgentBuiltinMcpToolApproval {
    mycopilot_core::AgentBuiltinMcpToolApproval {
        schema_version: mycopilot_core::BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION,
        identity: mycopilot_core::BuiltinMcpToolApprovalIdentity {
            action_id: "action".to_string(),
            approval_id: "approval".to_string(),
            run_id: "run".to_string(),
            call_id: "call".to_string(),
            capability_id: "browser_automation".to_string(),
            capability_activation_id: "activation".to_string(),
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
            origin: Some("https://fixture.invalid".to_string()),
        },
        capability_display_name: "Browser automation".to_string(),
        tool_display_name: "Evaluate page script".to_string(),
        call_reason: "Run the reviewed page script.".to_string(),
        operation_category: "page_script_execution".to_string(),
        resource_summary: mycopilot_core::BuiltinMcpToolResourceSummary {
            scope: "page_script_execution".to_string(),
            display_name: "Current managed page".to_string(),
            file_basenames: Vec::new(),
            origin: Some("https://fixture.invalid".to_string()),
        },
        risk_kinds: vec![mycopilot_core::BuiltinMcpToolRiskKind::PageScriptExecution],
        created_at: 1,
        expires_at: 901,
        approval_status: AgentApprovalStatus::Approved,
    }
}

#[test]
fn maps_mcp_outcomes_to_protocol_is_error_semantics() {
    for outcome in [
        AgentMcpToolInvocationOutcome::ToolError,
        AgentMcpToolInvocationOutcome::OutputTooLarge,
        AgentMcpToolInvocationOutcome::TransportError,
        AgentMcpToolInvocationOutcome::TimedOut,
        AgentMcpToolInvocationOutcome::PayloadUnavailable,
    ] {
        assert_eq!(mcp_lifecycle_is_error(outcome), Some(true));
    }
    assert_eq!(
        mcp_lifecycle_is_error(AgentMcpToolInvocationOutcome::Succeeded),
        Some(false)
    );
    for outcome in [
        AgentMcpToolInvocationOutcome::Cancelled,
        AgentMcpToolInvocationOutcome::Rejected,
        AgentMcpToolInvocationOutcome::Expired,
        AgentMcpToolInvocationOutcome::PolicyDenied,
        AgentMcpToolInvocationOutcome::OutcomeUnknown,
    ] {
        assert_eq!(mcp_lifecycle_is_error(outcome), None);
    }
}

#[test]
fn failure_retry_and_stage_are_bound_to_dispatch_evidence() {
    assert!(mcp_failure_is_retryable(
        "mcp.tool_snapshot_stale",
        AgentMcpDispatchCertainty::DefinitelyNotDispatched,
    ));
    assert!(!mcp_failure_is_retryable(
        "mcp.tool_snapshot_stale",
        AgentMcpDispatchCertainty::PossiblyDispatched,
    ));
    assert!(!mcp_failure_is_retryable(
        "mcp.tool_output_too_large",
        AgentMcpDispatchCertainty::DefinitelyNotDispatched,
    ));
    assert_eq!(
        mcp_invocation_failure_stage(false, AgentMcpDispatchCertainty::DefinitelyNotDispatched,),
        AgentMcpInvocationFailureStage::Dispatch
    );
    assert_eq!(
        mcp_invocation_failure_stage(false, AgentMcpDispatchCertainty::PossiblyDispatched),
        AgentMcpInvocationFailureStage::Transport
    );
    assert_eq!(
        mcp_invocation_failure_stage(false, AgentMcpDispatchCertainty::ResponseReceived),
        AgentMcpInvocationFailureStage::ServerResponse
    );
    assert_eq!(
        mcp_invocation_failure_stage(true, AgentMcpDispatchCertainty::PossiblyDispatched),
        AgentMcpInvocationFailureStage::Preflight
    );
}

#[tokio::test]
async fn supervised_builtin_mcp_invocation_returns_authoritative_success() {
    let approval = builtin_mcp_approval();
    let result = supervised_builtin_mcp_tool_result(&approval, Duration::from_secs(1), async {
        Ok(serde_json::json!({ "value": "fixture" }))
    })
    .await;

    assert!(result.ok);
    assert_eq!(
        result
            .result
            .as_ref()
            .and_then(|value| value.get("value"))
            .and_then(Value::as_str),
        Some("fixture")
    );
}

#[tokio::test]
async fn supervised_builtin_mcp_invocation_timeout_is_outcome_unknown() {
    let approval = builtin_mcp_approval();
    let result = supervised_builtin_mcp_tool_result(
        &approval,
        Duration::from_millis(1),
        std::future::pending::<AgentResult<Value>>(),
    )
    .await;

    assert!(!result.ok);
    assert_eq!(
        result
            .result
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str),
        Some("outcome_unknown")
    );
}

#[tokio::test]
async fn supervised_builtin_mcp_invocation_panic_is_outcome_unknown() {
    let approval = builtin_mcp_approval();
    let result = supervised_builtin_mcp_tool_result(&approval, Duration::from_secs(1), async {
        panic!("fixture provider panic")
    })
    .await;

    assert!(!result.ok);
    assert_eq!(
        result
            .result
            .as_ref()
            .and_then(|value| value.get("dispatchCertainty"))
            .and_then(Value::as_str),
        Some("possibly_dispatched")
    );
}

#[test]
fn skill_script_setup_failure_is_explicitly_pre_execution_and_retryable() {
    let result = skill_script_setup_failure_result(
        "skill-call",
        "executionSetupFailed",
        "retry",
        "fixture setup failure",
    );
    let details = result.result.expect("setup failure is structured");

    assert!(!result.ok);
    assert_eq!(details["status"], "failed");
    assert_eq!(details["outcome"], "definitely_not_executed");
    assert_eq!(details["effectsMayHaveOccurred"], false);
    assert_eq!(details["retryable"], true);
}

#[tokio::test]
async fn skill_script_worker_panic_is_observed_by_supervisor_join() {
    let joined = join_skill_script_worker(async {
        panic!("fixture Skill script worker panic");
    })
    .await;

    assert!(joined
        .expect_err("worker panic must be observed")
        .is_panic());
}

#[test]
fn skill_script_worker_failure_after_effect_boundary_is_outcome_unknown() {
    let result = skill_script_worker_failure_result("skill-call", true, false);
    let details = result.result.expect("worker failure is structured");

    assert!(!result.ok);
    assert_eq!(details["status"], "outcome_unknown");
    assert_eq!(details["outcome"], "outcome_unknown");
    assert_eq!(details["code"], "skill_script.outcome_unknown");
    assert_eq!(details["effectsMayHaveOccurred"], true);
    assert_eq!(details["retryable"], false);

    let pre_effect = skill_script_worker_failure_result("skill-call", false, false);
    let pre_effect_details = pre_effect.result.expect("worker failure is structured");
    assert_eq!(pre_effect_details["status"], "failed");
    assert_eq!(pre_effect_details["outcome"], "definitely_not_executed");
    assert_eq!(pre_effect_details["effectsMayHaveOccurred"], false);
    assert_eq!(pre_effect_details["retryable"], true);
}

#[test]
fn durable_unknown_receipt_clears_worker_file_effect_fence() {
    let tracker = Arc::new(FileEffectTracker::default());
    let conversation_id = "conversation-skill-worker";
    let run_id = "run-skill-worker";
    let effect_id = "effect-skill-worker";
    {
        let mut worker_guard = tracker.register(None, Some(conversation_id), run_id, effect_id);
        worker_guard.mark_effects_started();
    }
    assert_eq!(
        tracker.unsettled_effect_ids_for_conversation(conversation_id),
        vec![format!("{run_id}/{effect_id}")]
    );

    {
        let mut settlement_guard = tracker.register(None, Some(conversation_id), run_id, effect_id);
        settlement_guard.mark_durably_settled();
    }
    assert!(tracker
        .unsettled_effect_ids_for_conversation(conversation_id)
        .is_empty());
    assert!(tracker
        .active_run_ids_for_conversation(conversation_id)
        .is_empty());
}
