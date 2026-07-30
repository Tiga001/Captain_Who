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
    let approval = propose(&registry, model_name, json!({"input": secret})).unwrap();

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
    let rendered = serde_json::to_string(&pending).unwrap();
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("input"));

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
        },
    )
    .is_err());
}
