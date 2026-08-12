use super::*;

#[test]
fn lifecycle_event_contains_only_bounded_safe_fields() {
    let model_name = "mcp__fixture__lifecycle";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "lifecycle",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let secret = "lifecycle-private-value";
    let approval = propose(
        &registry,
        model_name,
        json!({
            "input": secret,
            "call_reason": "Inspect the fixture metadata."
        }),
    )
    .unwrap();

    let pending = mcp_tool_invocation_event(
        &approval,
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
    )
    .unwrap();
    assert!(pending.external);
    assert_eq!(
        pending.dispatch_certainty,
        AgentMcpDispatchCertainty::DefinitelyNotDispatched
    );
    assert_eq!(pending.action_id, approval.identity.action_id);
    assert_eq!(pending.invocation_id, approval.identity.invocation_id);
    assert_eq!(
        pending.display_reason.as_deref(),
        Some("Inspect the fixture metadata.")
    );
    let rendered = serde_json::to_string(&pending).unwrap();
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("input"));
    let diagnostics = pending.diagnostics.as_ref().unwrap();
    assert_eq!(diagnostics.schema_version, 1);
    assert!(diagnostics.argument_encoded_bytes > 0);
    assert!(diagnostics.argument_value_count > 0);
    assert!(diagnostics.result.is_none());
    assert!(diagnostics.failure_stage.is_none());

    for required_field in ["displayReason", "diagnostics"] {
        let mut missing_required = serde_json::to_value(&pending).unwrap();
        missing_required
            .as_object_mut()
            .unwrap()
            .remove(required_field);
        assert!(
            serde_json::from_value::<AgentMcpToolInvocationEvent>(missing_required).is_err(),
            "missing current field {required_field} must fail closed"
        );
    }

    for (state, outcome, is_error) in [
        (
            AgentMcpToolInvocationState::Completed,
            AgentMcpToolInvocationOutcome::Succeeded,
            Some(false),
        ),
        (
            AgentMcpToolInvocationState::Completed,
            AgentMcpToolInvocationOutcome::ToolError,
            Some(true),
        ),
        (
            AgentMcpToolInvocationState::Failed,
            AgentMcpToolInvocationOutcome::OutputTooLarge,
            Some(true),
        ),
        (
            AgentMcpToolInvocationState::Failed,
            AgentMcpToolInvocationOutcome::TransportError,
            Some(true),
        ),
        (
            AgentMcpToolInvocationState::Failed,
            AgentMcpToolInvocationOutcome::TimedOut,
            Some(true),
        ),
        (
            AgentMcpToolInvocationState::Cancelled,
            AgentMcpToolInvocationOutcome::Cancelled,
            None,
        ),
        (
            AgentMcpToolInvocationState::Rejected,
            AgentMcpToolInvocationOutcome::Rejected,
            None,
        ),
        (
            AgentMcpToolInvocationState::Expired,
            AgentMcpToolInvocationOutcome::Expired,
            None,
        ),
        (
            AgentMcpToolInvocationState::PayloadUnavailable,
            AgentMcpToolInvocationOutcome::PayloadUnavailable,
            Some(true),
        ),
        (
            AgentMcpToolInvocationState::PolicyDenied,
            AgentMcpToolInvocationOutcome::PolicyDenied,
            None,
        ),
        (
            AgentMcpToolInvocationState::OutcomeUnknown,
            AgentMcpToolInvocationOutcome::OutcomeUnknown,
            None,
        ),
    ] {
        let dispatch_certainty = match (state, outcome) {
            (
                AgentMcpToolInvocationState::Completed,
                AgentMcpToolInvocationOutcome::Succeeded | AgentMcpToolInvocationOutcome::ToolError,
            )
            | (
                AgentMcpToolInvocationState::Failed,
                AgentMcpToolInvocationOutcome::OutputTooLarge,
            ) => AgentMcpDispatchCertainty::ResponseReceived,
            (
                AgentMcpToolInvocationState::OutcomeUnknown,
                AgentMcpToolInvocationOutcome::OutcomeUnknown,
            ) => AgentMcpDispatchCertainty::PossiblyDispatched,
            _ => AgentMcpDispatchCertainty::DefinitelyNotDispatched,
        };
        let result_size = (state == AgentMcpToolInvocationState::Completed).then_some(
            AgentMcpResultSizeSummary {
                content_block_count: 1,
                text_bytes: 7,
                structured_bytes: 0,
                omitted_block_count: 0,
                omitted_encoded_bytes: 0,
            },
        );
        let failure_stage = match outcome {
            AgentMcpToolInvocationOutcome::Succeeded | AgentMcpToolInvocationOutcome::Rejected => {
                None
            }
            AgentMcpToolInvocationOutcome::ToolError => {
                Some(AgentMcpInvocationFailureStage::ServerResponse)
            }
            AgentMcpToolInvocationOutcome::OutputTooLarge => {
                Some(AgentMcpInvocationFailureStage::ResultProjection)
            }
            AgentMcpToolInvocationOutcome::Expired
            | AgentMcpToolInvocationOutcome::PayloadUnavailable => {
                Some(AgentMcpInvocationFailureStage::ApprovalPayload)
            }
            AgentMcpToolInvocationOutcome::PolicyDenied => {
                Some(AgentMcpInvocationFailureStage::Policy)
            }
            AgentMcpToolInvocationOutcome::Cancelled | AgentMcpToolInvocationOutcome::TimedOut => {
                Some(AgentMcpInvocationFailureStage::Preflight)
            }
            AgentMcpToolInvocationOutcome::TransportError
            | AgentMcpToolInvocationOutcome::OutcomeUnknown => {
                Some(AgentMcpInvocationFailureStage::Transport)
            }
        };
        mcp_tool_invocation_event(
            &approval,
            McpToolInvocationEventUpdate {
                state,
                dispatch_certainty,
                outcome: Some(outcome),
                is_error,
                error_code: (outcome != AgentMcpToolInvocationOutcome::Succeeded)
                    .then_some("mcp.lifecycle"),
                duration_ms: Some(17),
                output_truncated: dispatch_certainty == AgentMcpDispatchCertainty::ResponseReceived,
                result_size,
                failure_stage,
            },
        )
        .unwrap();
    }
    mcp_tool_invocation_event(
        &approval,
        McpToolInvocationEventUpdate {
            state: AgentMcpToolInvocationState::Approved,
            dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            outcome: None,
            is_error: None,
            error_code: None,
            duration_ms: None,
            output_truncated: false,
            result_size: None,
            failure_stage: None,
        },
    )
    .unwrap();
    assert!(mcp_tool_invocation_event(
        &approval,
        McpToolInvocationEventUpdate {
            state: AgentMcpToolInvocationState::Failed,
            dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            outcome: Some(AgentMcpToolInvocationOutcome::TransportError),
            is_error: Some(true),
            error_code: Some("unsafe server message"),
            duration_ms: Some(17),
            output_truncated: false,
            result_size: None,
            failure_stage: Some(AgentMcpInvocationFailureStage::Transport),
        },
    )
    .is_err());
    assert!(mcp_tool_invocation_event(
        &approval,
        McpToolInvocationEventUpdate {
            state: AgentMcpToolInvocationState::Completed,
            dispatch_certainty: AgentMcpDispatchCertainty::ResponseReceived,
            outcome: Some(AgentMcpToolInvocationOutcome::Rejected),
            is_error: None,
            error_code: None,
            duration_ms: Some(17),
            output_truncated: false,
            result_size: None,
            failure_stage: None,
        },
    )
    .is_err());
    assert!(mcp_tool_invocation_event(
        &approval,
        McpToolInvocationEventUpdate {
            state: AgentMcpToolInvocationState::PendingApproval,
            dispatch_certainty: AgentMcpDispatchCertainty::PossiblyDispatched,
            outcome: None,
            is_error: None,
            error_code: None,
            duration_ms: None,
            output_truncated: false,
            result_size: None,
            failure_stage: None,
        },
    )
    .is_err());
    assert!(mcp_tool_invocation_event(
        &approval,
        McpToolInvocationEventUpdate {
            state: AgentMcpToolInvocationState::Failed,
            dispatch_certainty: AgentMcpDispatchCertainty::ResponseReceived,
            outcome: Some(AgentMcpToolInvocationOutcome::TimedOut),
            is_error: Some(true),
            error_code: Some("mcp.tool_timeout"),
            duration_ms: Some(17),
            output_truncated: false,
            result_size: None,
            failure_stage: Some(AgentMcpInvocationFailureStage::Transport),
        },
    )
    .is_err());
}
