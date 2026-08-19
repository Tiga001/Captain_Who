use super::*;

fn failed_mcp_tool_result(
    approval: &mycopilot_core::AgentMcpToolApproval,
    code: &str,
    message: &str,
    dispatch_certainty: AgentMcpDispatchCertainty,
) -> AgentToolResult {
    let (status, outcome) = match code {
        "mcp.tool_outcome_unknown" => ("outcome_unknown", "outcome_unknown"),
        "mcp.tool_output_too_large" => ("failed", "output_too_large"),
        "mcp.approval_payload_expired" => ("expired", "expired"),
        "mcp.approval_payload_unavailable" => ("payload_unavailable", "payload_unavailable"),
        "mcp.approval_policy_denied" => ("policy_denied", "policy_denied"),
        "mcp.tool_cancelled_before_dispatch" => ("cancelled", "cancelled"),
        "mcp.tool_timeout" => ("failed", "timed_out"),
        _ => ("failed", "transport_error"),
    };
    let retryable = mcp_failure_is_retryable(code, dispatch_certainty);
    AgentToolResult {
        exact_archive_file: None,
        call_id: approval.identity.call_id.clone(),
        tool: approval.identity.provenance.model_tool_name.clone(),
        ok: false,
        result: Some(serde_json::json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "status": status,
            "outcome": outcome,
            "code": code,
            "retryable": retryable,
            "external": true,
            "dispatchCertainty": mcp_dispatch_certainty_label(dispatch_certainty),
            "isError": true,
        })),
        error: Some(message.to_string()),
    }
}

fn mcp_failure_is_retryable(code: &str, certainty: AgentMcpDispatchCertainty) -> bool {
    certainty == AgentMcpDispatchCertainty::DefinitelyNotDispatched
        && matches!(
            code,
            "mcp.tool_timeout" | "mcp.tool_snapshot_stale" | "mcp.tool_unavailable"
        )
}

fn persisted_mcp_tool_result(result: &AgentToolResult) -> AgentToolResult {
    mycopilot_core::mcp_tool_result_persistence_projection(result)
}

fn persisted_builtin_mcp_tool_result(result: &AgentToolResult) -> AgentToolResult {
    mycopilot_core::builtin_capability_tool_result_persistence_projection(result)
}

fn mcp_tool_result_output_truncated(result: &AgentToolResult) -> bool {
    result
        .result
        .as_ref()
        .and_then(|value| value.get("truncatedAtSource"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn mcp_lifecycle_is_error(outcome: AgentMcpToolInvocationOutcome) -> Option<bool> {
    match outcome {
        AgentMcpToolInvocationOutcome::Succeeded => Some(false),
        AgentMcpToolInvocationOutcome::ToolError
        | AgentMcpToolInvocationOutcome::OutputTooLarge
        | AgentMcpToolInvocationOutcome::TransportError
        | AgentMcpToolInvocationOutcome::TimedOut
        | AgentMcpToolInvocationOutcome::PayloadUnavailable => Some(true),
        AgentMcpToolInvocationOutcome::Cancelled
        | AgentMcpToolInvocationOutcome::Rejected
        | AgentMcpToolInvocationOutcome::Expired
        | AgentMcpToolInvocationOutcome::PolicyDenied
        | AgentMcpToolInvocationOutcome::OutcomeUnknown => None,
    }
}

fn mcp_agent_error_dispatch_certainty(error: &AgentError) -> AgentMcpDispatchCertainty {
    match error
        .details()
        .and_then(|details| details.get("dispatchCertainty"))
        .and_then(serde_json::Value::as_str)
    {
        Some("definitely_not_dispatched") => AgentMcpDispatchCertainty::DefinitelyNotDispatched,
        Some("response_received") => AgentMcpDispatchCertainty::ResponseReceived,
        Some("possibly_dispatched") => AgentMcpDispatchCertainty::PossiblyDispatched,
        _ => AgentMcpDispatchCertainty::PossiblyDispatched,
    }
}

fn mcp_dispatch_certainty_label(certainty: AgentMcpDispatchCertainty) -> &'static str {
    match certainty {
        AgentMcpDispatchCertainty::DefinitelyNotDispatched => "definitely_not_dispatched",
        AgentMcpDispatchCertainty::PossiblyDispatched => "possibly_dispatched",
        AgentMcpDispatchCertainty::ResponseReceived => "response_received",
    }
}

fn emit_mcp_lifecycle_event(
    notifications: &CoreServerNotificationSender,
    run_id: &str,
    approval: &mycopilot_core::AgentMcpToolApproval,
    update: McpToolInvocationEventUpdate<'_>,
) {
    match mcp_tool_invocation_event(approval, update) {
        Ok(invocation) => {
            record_safe_mcp_invocation_event(&invocation);
            let _ = notifications.send(agent_event_notification(
                AgentEvent::McpToolInvocationStateChanged {
                    run_id: run_id.to_string(),
                    invocation,
                },
            ));
        }
        Err(_) => {
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id.to_string()),
                trace_sequence: None,
                message: "The MCP invocation lifecycle could not be projected safely.".to_string(),
                recoverable: false,
                code: Some("mcp.lifecycle_projection_failed".to_string()),
                details: None,
            }));
        }
    }
}

fn record_safe_mcp_invocation_event(invocation: &mycopilot_core::AgentMcpToolInvocationEvent) {
    let diagnostics = invocation.diagnostics.as_ref();
    let result = diagnostics.and_then(|diagnostics| diagnostics.result.as_ref());
    tracing::info!(
        target: "mycopilot_core_server::mcp_invocation",
        invocation_id = %invocation.invocation_id,
        server_id = %invocation.server_id,
        state = ?invocation.state,
        outcome = ?invocation.outcome,
        dispatch_certainty = ?invocation.dispatch_certainty,
        duration_ms = invocation.duration_ms,
        error_code = invocation.error_code.as_deref(),
        output_truncated = invocation.output_truncated,
        argument_encoded_bytes = diagnostics.map(|value| value.argument_encoded_bytes),
        argument_value_count = diagnostics.map(|value| value.argument_value_count),
        argument_max_depth = diagnostics.map(|value| value.argument_max_depth),
        result_content_blocks = result.map(|value| value.content_block_count),
        result_text_bytes = result.map(|value| value.text_bytes),
        result_structured_bytes = result.map(|value| value.structured_bytes),
        result_omitted_blocks = result.map(|value| value.omitted_block_count),
        failure_stage = ?diagnostics.and_then(|value| value.failure_stage),
        "MCP invocation lifecycle transition"
    );
}

struct McpInvocationSettlement {
    tool_result: AgentToolResult,
    state: AgentMcpToolInvocationState,
    outcome: AgentMcpToolInvocationOutcome,
    error_code: Option<String>,
    dispatch_certainty: AgentMcpDispatchCertainty,
    output_truncated: bool,
    result_size: Option<AgentMcpResultSizeSummary>,
    failure_stage: Option<AgentMcpInvocationFailureStage>,
}

fn settle_mcp_invocation(
    approval: &mycopilot_core::AgentMcpToolApproval,
    invocation_result: AgentResult<mycopilot_core::McpToolInvocationResult>,
    failed_before_dispatch: bool,
) -> McpInvocationSettlement {
    let (
        tool_result,
        state,
        outcome,
        error_code,
        dispatch_certainty,
        output_truncated,
        result_size,
        failure_stage,
    ) = match invocation_result {
        Ok(result) => {
            let is_error = result.is_error;
            let result_size = mcp_tool_result_size_summary(&result).ok();
            match mcp_tool_result_from_approved_invocation(approval, &result) {
                Ok(tool_result) => {
                    let output_truncated =
                        result.truncated_at_source || mcp_tool_result_output_truncated(&tool_result);
                    (
                        tool_result,
                        AgentMcpToolInvocationState::Completed,
                        if is_error {
                            AgentMcpToolInvocationOutcome::ToolError
                        } else {
                            AgentMcpToolInvocationOutcome::Succeeded
                        },
                        is_error.then(|| "mcp.tool_error".to_string()),
                        AgentMcpDispatchCertainty::ResponseReceived,
                        output_truncated,
                        result_size,
                        is_error.then_some(AgentMcpInvocationFailureStage::ServerResponse),
                    )
                }
                Err(error) => (
                    failed_mcp_tool_result(
                        approval,
                        "mcp.result_projection_failed",
                        "The MCP response could not be projected safely.",
                        AgentMcpDispatchCertainty::ResponseReceived,
                    ),
                    AgentMcpToolInvocationState::Failed,
                    AgentMcpToolInvocationOutcome::TransportError,
                    Some(
                        error
                            .code()
                            .unwrap_or("mcp.result_projection_failed")
                            .to_string(),
                    ),
                    AgentMcpDispatchCertainty::ResponseReceived,
                    result.truncated_at_source,
                    None,
                    Some(AgentMcpInvocationFailureStage::ResultProjection),
                ),
            }
        }
        Err(error) if error.code() == Some("mcp.tool_outcome_unknown") => (
            failed_mcp_tool_result(
                approval,
                "mcp.tool_outcome_unknown",
                "The MCP invocation may have reached the server, but its outcome is unknown. Check the authoritative system before deciding whether to try again.",
                AgentMcpDispatchCertainty::PossiblyDispatched,
            ),
            AgentMcpToolInvocationState::OutcomeUnknown,
            AgentMcpToolInvocationOutcome::OutcomeUnknown,
            Some("mcp.tool_outcome_unknown".to_string()),
            AgentMcpDispatchCertainty::PossiblyDispatched,
            false,
            None,
            Some(AgentMcpInvocationFailureStage::Transport),
        ),
        Err(error) if error.code() == Some("mcp.tool_output_too_large") => (
            failed_mcp_tool_result(
                approval,
                "mcp.tool_output_too_large",
                "The MCP server returned a Tool response that exceeded Host output limits.",
                AgentMcpDispatchCertainty::ResponseReceived,
            ),
            AgentMcpToolInvocationState::Failed,
            AgentMcpToolInvocationOutcome::OutputTooLarge,
            Some("mcp.tool_output_too_large".to_string()),
            AgentMcpDispatchCertainty::ResponseReceived,
            true,
            None,
            Some(AgentMcpInvocationFailureStage::ResultProjection),
        ),
        Err(error) if error.code() == Some("mcp.approval_payload_expired") => (
            failed_mcp_tool_result(
                approval,
                "mcp.approval_payload_expired",
                "The MCP invocation payload expired before dispatch.",
                AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            ),
            AgentMcpToolInvocationState::Expired,
            AgentMcpToolInvocationOutcome::Expired,
            Some("mcp.approval_payload_expired".to_string()),
            AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            false,
            None,
            Some(AgentMcpInvocationFailureStage::ApprovalPayload),
        ),
        Err(error)
            if matches!(
                error.code(),
                Some("mcp.approval_payload_unavailable" | "mcp.approval_payload_store_unavailable")
            ) =>
        {
            (
                failed_mcp_tool_result(
                    approval,
                    "mcp.approval_payload_unavailable",
                    "The sealed MCP invocation payload is unavailable.",
                    AgentMcpDispatchCertainty::DefinitelyNotDispatched,
                ),
                AgentMcpToolInvocationState::PayloadUnavailable,
                AgentMcpToolInvocationOutcome::PayloadUnavailable,
                Some("mcp.approval_payload_unavailable".to_string()),
                AgentMcpDispatchCertainty::DefinitelyNotDispatched,
                false,
                None,
                Some(AgentMcpInvocationFailureStage::ApprovalPayload),
            )
        }
        Err(error) if error.code() == Some("mcp.approval_policy_denied") => (
            failed_mcp_tool_result(
                approval,
                "mcp.approval_policy_denied",
                "Host policy denied the MCP invocation.",
                AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            ),
            AgentMcpToolInvocationState::PolicyDenied,
            AgentMcpToolInvocationOutcome::PolicyDenied,
            Some("mcp.approval_policy_denied".to_string()),
            AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            false,
            None,
            Some(AgentMcpInvocationFailureStage::Policy),
        ),
        Err(error) if failed_before_dispatch && error.is_cancelled() => (
            failed_mcp_tool_result(
                approval,
                "mcp.tool_cancelled_before_dispatch",
                "The MCP invocation was cancelled before dispatch.",
                AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            ),
            AgentMcpToolInvocationState::Cancelled,
            AgentMcpToolInvocationOutcome::Cancelled,
            Some("mcp.tool_cancelled".to_string()),
            AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            false,
            None,
            Some(AgentMcpInvocationFailureStage::Preflight),
        ),
        Err(error) => {
            let dispatch_certainty = if failed_before_dispatch {
                AgentMcpDispatchCertainty::DefinitelyNotDispatched
            } else {
                mcp_agent_error_dispatch_certainty(&error)
            };
            if dispatch_certainty == AgentMcpDispatchCertainty::PossiblyDispatched {
                (
                    failed_mcp_tool_result(
                        approval,
                        "mcp.tool_outcome_unknown",
                        "The MCP invocation may have reached the server, but its outcome is unknown. Check the authoritative system before deciding whether to try again.",
                        dispatch_certainty,
                    ),
                    AgentMcpToolInvocationState::OutcomeUnknown,
                    AgentMcpToolInvocationOutcome::OutcomeUnknown,
                    Some("mcp.tool_outcome_unknown".to_string()),
                    dispatch_certainty,
                    false,
                    None,
                    Some(AgentMcpInvocationFailureStage::Transport),
                )
            } else {
                let outcome = if error.code() == Some("mcp.tool_timeout")
                    && dispatch_certainty == AgentMcpDispatchCertainty::DefinitelyNotDispatched
                {
                    AgentMcpToolInvocationOutcome::TimedOut
                } else {
                    AgentMcpToolInvocationOutcome::TransportError
                };
                (
                    failed_mcp_tool_result(
                        approval,
                        error.code().unwrap_or("mcp.tool_failed"),
                        "The MCP invocation failed without a server Tool response.",
                        dispatch_certainty,
                    ),
                    AgentMcpToolInvocationState::Failed,
                    outcome,
                    Some(error.code().unwrap_or("mcp.tool_failed").to_string()),
                    dispatch_certainty,
                    false,
                    None,
                    Some(mcp_invocation_failure_stage(
                        failed_before_dispatch,
                        dispatch_certainty,
                    )),
                )
            }
        }
    };
    McpInvocationSettlement {
        tool_result,
        state,
        outcome,
        error_code,
        dispatch_certainty,
        output_truncated,
        result_size,
        failure_stage,
    }
}

fn mcp_invocation_failure_stage(
    failed_before_dispatch: bool,
    dispatch_certainty: AgentMcpDispatchCertainty,
) -> AgentMcpInvocationFailureStage {
    if failed_before_dispatch {
        return AgentMcpInvocationFailureStage::Preflight;
    }
    match dispatch_certainty {
        AgentMcpDispatchCertainty::DefinitelyNotDispatched => {
            AgentMcpInvocationFailureStage::Dispatch
        }
        AgentMcpDispatchCertainty::PossiblyDispatched => AgentMcpInvocationFailureStage::Transport,
        AgentMcpDispatchCertainty::ResponseReceived => {
            AgentMcpInvocationFailureStage::ServerResponse
        }
    }
}

impl AgentService {
    pub(in crate::application::agent) fn execute_office_operation(
        &self,
        agent_input: &AgentChatInput,
        office_operation: &mycopilot_core::AgentOfficeOperationRequest,
        skill_resources: Option<Arc<SkillResourceSession>>,
        cancellation_token: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> AgentToolResult {
        let tool = office_tool_name(office_operation.prepared.request.document_kind);
        if office_operation.approval_status != AgentApprovalStatus::Approved {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: office_operation.id.clone(),
                tool: tool.to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation_policy",
                    "code": "notAuthorized",
                })),
                error: Some("Office operation has not been authorized.".to_string()),
            };
        }
        if office_operation.schema_version != mycopilot_core::AGENT_OFFICE_OPERATION_SCHEMA_VERSION
            || !mycopilot_core::is_valid_agent_office_reason(&office_operation.reason)
            || mycopilot_core::validate_frozen_agent_office_semantic_args(office_operation).is_err()
            || office_operation.prepared.access
                != mycopilot_core::office::OfficeOperationAccess::FileWrite
            || office_operation.prepared.request.access()
                != mycopilot_core::office::OfficeOperationAccess::FileWrite
        {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: office_operation.id.clone(),
                tool: tool.to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation_policy",
                    "code": "invalidApprovedSnapshot",
                    "recovery": "retry",
                })),
                error: Some(
                    "The approved Office action is not a supported file-write snapshot."
                        .to_string(),
                ),
            };
        }
        if permissions_from_input(agent_input).write == mycopilot_core::AgentWritePermission::Denied
        {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: office_operation.id.clone(),
                tool: tool.to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation_policy",
                    "code": "writePermissionDenied",
                    "recovery": "changePermissions",
                })),
                error: Some(
                    "The current permission policy does not allow Office file changes.".to_string(),
                ),
            };
        }
        // Rebuild the Host-owned execution context at the last responsible moment. The Office
        // engine re-resolves every frozen path against these current run-scoped permissions and
        // attachment capabilities before it creates staging or invokes the provider.
        let skill_resources = match skill_resources {
            Some(resources) => Some(resources),
            None => match self.restore_skill_resource_session(agent_input) {
                Ok(resources) => resources,
                Err(error) => {
                    return office_skill_resource_restore_failure(
                        office_operation,
                        tool,
                        &error.to_string(),
                    );
                }
            },
        };
        let file_inputs = agent_file_input_execution_context(
            agent_input,
            skill_resources,
            Arc::clone(&self.storage),
        );
        let execution_context = mycopilot_core::office::OfficeExecutionContext::from_run_context(
            agent_input.context.as_ref(),
        )
        .with_file_inputs(file_inputs);
        match self.office_engine.execute_prepared(
            &execution_context,
            &office_operation.prepared,
            cancellation_token,
            action_cancel_flag,
        ) {
            Ok(result) => office_operation_tool_result(&office_operation.id, tool, result),
            Err(error) => office_engine_failure(
                &office_operation.id,
                tool,
                &office_operation.prepared,
                error,
            ),
        }
    }

    pub(in crate::application::agent) fn execute_skill_script(
        &self,
        agent_input: &AgentChatInput,
        script: &AgentSkillScriptRequest,
        resources: Option<&SkillResourceSession>,
        authorization_source: CommandAuthorizationSource,
        cancellation_token: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> AgentToolResult {
        if script.approval_status != AgentApprovalStatus::Approved {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script_policy",
                    "code": "notAuthorized",
                })),
                error: Some("Skill script execution has not been authorized.".to_string()),
            };
        }
        let permissions = permissions_from_input(agent_input);
        let authorized = permissions.read == mycopilot_core::AgentReadPermission::All
            && permissions.write == mycopilot_core::AgentWritePermission::All
            && permissions.command_safety == mycopilot_core::AgentCommandSafetyPolicy::FullAccess
            && match authorization_source {
                CommandAuthorizationSource::Automatic => false,
                CommandAuthorizationSource::ExplicitUser => true,
            };
        if !authorized {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script_policy",
                    "code": "authorizationDenied",
                    "authorizationSource": authorization_source,
                })),
                error: Some(
                    "The current permission policy does not authorize this Skill script."
                        .to_string(),
                ),
            };
        }
        let Some(resources) = resources else {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script",
                    "code": "snapshotUnavailable",
                    "recovery": "reactivateSkill",
                })),
                error: Some("The activated Skill resource snapshot is unavailable.".to_string()),
            };
        };
        let Some(workspace_root) = workspace_root_optional(agent_input) else {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script",
                    "code": "workspaceUnavailable",
                    "recovery": "selectWorkspace",
                })),
                error: Some("Skill script execution requires a workspace.".to_string()),
            };
        };
        match execute_skill_python_script(
            resources,
            &workspace_root,
            script,
            cancellation_token,
            action_cancel_flag,
        ) {
            Ok(result) => skill_script_tool_result(&script.id, result),
            Err(error) => skill_script_runtime_failure(&script.id, error),
        }
    }

    pub(in crate::application::agent) fn execute_skill_materialization(
        &self,
        agent_input: &AgentChatInput,
        materialization: &AgentSkillMaterializationRequest,
        resources: Option<&SkillResourceSession>,
    ) -> AgentToolResult {
        let execute = || -> Result<AgentSkillMaterializationResult, (String, Option<Value>)> {
            let plain_error = |message: &str| (message.to_string(), None);
            if materialization.approval_status != AgentApprovalStatus::Approved {
                return Err(plain_error(
                    "Skill resource materialization has not been authorized.",
                ));
            }
            if permissions_from_input(agent_input).write
                == mycopilot_core::AgentWritePermission::Denied
            {
                return Err(plain_error(
                    "Skill resource materialization requires workspace write permission.",
                ));
            }
            let resources = resources.ok_or_else(|| {
                plain_error("The activated Skill resource snapshot is unavailable.")
            })?;
            let workspace_root = workspace_root_optional(agent_input).ok_or_else(|| {
                plain_error("Skill resource materialization requires a workspace.")
            })?;
            let destination =
                SkillMaterializationDestination::parse(materialization.destination.clone())
                    .map_err(materialization_failure)?;
            let materializer = SkillResourceMaterializer::new();
            let result = if let Some(source_prefix) = materialization.source_prefix.as_deref() {
                let source = SkillPackageUri::parse(&materialization.source_uri)
                    .map_err(|error| plain_error(&error.to_string()))?;
                let source_prefix = SkillResourcePath::parse(source_prefix.to_string())
                    .map_err(|error| plain_error(&error.to_string()))?;
                let request = SkillTemplateTreeMaterializationRequest::new(
                    source,
                    source_prefix,
                    workspace_root,
                    destination,
                )
                .map_err(materialization_failure)?;
                let outcome = materializer
                    .materialize_template_tree(resources, &request)
                    .map_err(materialization_failure)?;
                AgentSkillMaterializationResult {
                    status: materialization_result_status(outcome.status()),
                    source_uri: outcome.source().to_string(),
                    source_prefix: Some(outcome.source_prefix().to_string()),
                    destination: outcome.destination().to_string(),
                    source_revision: outcome.source().revision().as_str().to_string(),
                    file_count: u64::try_from(outcome.file_count()).unwrap_or(u64::MAX),
                    byte_count: outcome.byte_length(),
                    plan_digest: Some(outcome.plan_digest().to_string()),
                    error: None,
                    message: Some(materialization_success_message(outcome.status(), true)),
                }
            } else {
                let source = SkillResourceUri::parse(&materialization.source_uri)
                    .map_err(|error| plain_error(&error.to_string()))?;
                let request = SkillMaterializationRequest::new(source, workspace_root, destination)
                    .map_err(materialization_failure)?;
                let outcome = materializer
                    .materialize(resources, &request)
                    .map_err(materialization_failure)?;
                AgentSkillMaterializationResult {
                    status: materialization_result_status(outcome.status()),
                    source_uri: outcome.source().to_string(),
                    source_prefix: None,
                    destination: outcome.destination().to_string(),
                    source_revision: outcome.source().package().revision().as_str().to_string(),
                    file_count: 1,
                    byte_count: outcome.byte_length(),
                    plan_digest: Some(outcome.content_digest().to_string()),
                    error: None,
                    message: Some(materialization_success_message(outcome.status(), false)),
                }
            };
            Ok(result)
        };

        match execute() {
            Ok(result) => AgentToolResult {
                exact_archive_file: None,
                call_id: materialization.id.clone(),
                tool: "skills_materialize_resource".to_string(),
                ok: true,
                result: serde_json::to_value(result).ok(),
                error: None,
            },
            Err((error, structured)) => AgentToolResult {
                exact_archive_file: None,
                call_id: materialization.id.clone(),
                tool: "skills_materialize_resource".to_string(),
                ok: false,
                result: structured,
                error: Some(error),
            },
        }
    }

    pub(in crate::application::agent) fn queue_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let command_record = record.clone();
        tokio::spawn(async move {
            service
                .run_command_execution(command_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(in crate::application::agent) fn queue_mcp_tool_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_mcp_tool_execution(execution_record, call, guard, notifications, false)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(in crate::application::agent) fn queue_builtin_mcp_tool_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        grant: mycopilot_core::BuiltinMcpToolGrant,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_builtin_mcp_tool_execution(execution_record, call, grant, guard, notifications)
                .await;
        });
        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    async fn run_builtin_mcp_tool_execution(
        &self,
        mut record: PendingActionRecord,
        call: AgentToolCall,
        grant: mycopilot_core::BuiltinMcpToolGrant,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        let AgentProposedAction::BuiltinMcpToolApproval { mut approval } =
            record.snapshot.action.clone()
        else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            return;
        };
        approval.approval_status = AgentApprovalStatus::Approved;
        let Some(runtime) = self.builtin_capabilities.clone() else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            return;
        };
        let cancellation = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation.clone());
        if guard.cancel_flag().load(Ordering::SeqCst) {
            cancellation.cancel();
        }
        let _guard = guard;
        let result = if cancellation.is_cancelled() {
            let _ = runtime.revoke_builtin_mcp_tool_grant(&grant.grant_id, &grant.approval_id);
            mycopilot_core::builtin_mcp_tool_cancelled_result(&approval)
        } else {
            if self
                .transition_pending_status(&record, PendingActionStatus::Executing)
                .is_err()
            {
                let _ = runtime.revoke_builtin_mcp_tool_grant(&grant.grant_id, &grant.approval_id);
                self.unregister_cancellation_if_current(&run_id, &cancellation);
                return;
            }
            record.snapshot.status = PendingActionStatus::Executing;
            match runtime
                .invoke_approved_builtin_mcp_tool((*approval).clone(), grant, cancellation.clone())
                .await
            {
                Ok(value) => AgentToolResult {
                    exact_archive_file: None,
                    call_id: approval.identity.call_id.clone(),
                    tool: approval.identity.model_name.clone(),
                    ok: true,
                    result: Some(value),
                    error: None,
                },
                Err(error)
                    if error
                        .details()
                        .and_then(|value| value.get("dispatchCertainty"))
                        .and_then(Value::as_str)
                        == Some("possibly_dispatched") =>
                {
                    mycopilot_core::builtin_mcp_tool_outcome_unknown_result(&approval)
                }
                Err(error)
                    if error.is_cancelled()
                        && mcp_agent_error_dispatch_certainty(&error)
                            == AgentMcpDispatchCertainty::DefinitelyNotDispatched =>
                {
                    mycopilot_core::builtin_mcp_tool_cancelled_result(&approval)
                }
                Err(error) if error.is_cancelled() => {
                    mycopilot_core::builtin_mcp_tool_outcome_unknown_result(&approval)
                }
                Err(error) => AgentToolResult {
                    exact_archive_file: None,
                    call_id: approval.identity.call_id.clone(),
                    tool: approval.identity.model_name.clone(),
                    ok: false,
                    result: error.details().cloned().or_else(|| {
                        Some(serde_json::json!({
                            "schemaVersion": 1,
                            "type": "builtin_mcp_tool_approval",
                            "status": "failed",
                            "dispatchCertainty": "definitely_not_dispatched",
                            "contentOmitted": true,
                        }))
                    }),
                    error: Some(error.to_string()),
                },
            }
        };
        let final_pending_status = if result.ok {
            PendingActionStatus::Completed
        } else if result
            .result
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            == Some("cancelled")
        {
            PendingActionStatus::Cancelled
        } else {
            PendingActionStatus::Failed
        };
        let mut agent_input = record.agent_input.clone();
        agent_input.approval_decision = Some(AgentApprovalDecision {
            action_id: record.snapshot.action_id.clone(),
            status: AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        agent_input.tool_continuation = Some(AgentToolContinuation {
            call: call.clone(),
            result: result.clone(),
        });
        let mut persisted_agent_input = agent_input.clone();
        if let Some(continuation) = persisted_agent_input.tool_continuation.as_mut() {
            continuation.result = persisted_builtin_mcp_tool_result(&continuation.result);
        }
        if self
            .commit_audited_result_trace_with_continuation(
                &record,
                &persisted_agent_input,
                final_pending_status,
                None,
                now_ms(),
                &notifications,
            )
            .is_err()
        {
            self.unregister_cancellation_if_current(&run_id, &cancellation);
            return;
        }
        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(cancellation.clone()),
        )
        .await;
        self.unregister_cancellation_if_current(&run_id, &cancellation);
    }

    pub(in crate::application::agent) fn queue_claimed_mcp_tool_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_mcp_tool_execution(execution_record, call, guard, notifications, true)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(in crate::application::agent) async fn execute_auto_mcp_tool_action(
        &self,
        context: &AutoApprovedActionContext,
        approval: Box<mycopilot_core::AgentMcpToolApproval>,
        cancellation: AgentCancellationToken,
    ) -> AgentResult<AgentToolResult> {
        if approval.approval_mode != mycopilot_core::AgentMcpApprovalMode::Auto
            || approval.call.approval_status != AgentApprovalStatus::Approved
            || approval.identity.run_id != context.run_id
            || approval.identity.call_id != approval.call.id
            || approval.identity.provenance.model_tool_name != approval.call.tool
        {
            self.invalidate_mcp_pending_payload(&AgentProposedAction::McpToolCall {
                approval: approval.clone(),
            });
            return Err(AgentError::structured(
                "mcp.auto_invocation_not_authorized",
                "The MCP invocation is not authorized for automatic execution.",
                serde_json::json!({
                    "type": "mcp_tool",
                    "code": "autoInvocationNotAuthorized",
                    "retryable": false,
                    "dispatchCertainty": "definitely_not_dispatched",
                }),
            ));
        }
        let Some(invoker) = self.mcp_tool_invoker.clone() else {
            self.invalidate_mcp_pending_payload(&AgentProposedAction::McpToolCall {
                approval: approval.clone(),
            });
            return Err(AgentError::structured(
                "mcp.runtime_unavailable",
                "The MCP invocation runtime is unavailable.",
                serde_json::json!({
                    "type": "mcp_tool",
                    "code": "runtimeUnavailable",
                    "retryable": false,
                    "dispatchCertainty": "definitely_not_dispatched",
                }),
            ));
        };
        if self.is_agent_input_scope_deleting(&context.agent_input) {
            self.invalidate_mcp_pending_payload(&AgentProposedAction::McpToolCall {
                approval: approval.clone(),
            });
            return Err(AgentError::cancelled());
        }

        let action = AgentProposedAction::McpToolCall {
            approval: approval.clone(),
        };
        let mut journal = match self.prepare_auto_mcp_action_journal(
            &context.run_id,
            context.conversation_id.as_deref(),
            context.assistant_message_id.as_deref(),
            action.clone(),
            context.agent_input.clone(),
        ) {
            Ok(record) => record,
            Err(_) => {
                self.invalidate_mcp_pending_payload(&action);
                return Err(AgentError::structured(
                    "mcp.auto_dispatch_journal_unavailable",
                    "The MCP invocation could not establish its durable dispatch boundary.",
                    serde_json::json!({
                        "type": "mcp_tool",
                        "code": "autoDispatchJournalUnavailable",
                        "retryable": false,
                        "dispatchCertainty": "definitely_not_dispatched",
                    }),
                ));
            }
        };
        let preflight_result = if cancellation.is_cancelled() {
            Err(AgentError::cancelled())
        } else {
            invoker.revalidate_approved(&approval)
        };
        if preflight_result.is_err() {
            self.invalidate_mcp_pending_payload(&action);
        }

        let started_at = Instant::now();
        let failed_before_dispatch = preflight_result.is_err();
        if failed_before_dispatch {
            let invocation_result = preflight_result.map(|()| unreachable!());
            let settlement =
                settle_mcp_invocation(&approval, invocation_result, failed_before_dispatch);
            let terminal_outcome = if settlement.state == AgentMcpToolInvocationState::Cancelled {
                McpAutoActionJournalTerminalOutcome::Cancelled
            } else {
                McpAutoActionJournalTerminalOutcome::Failed
            };
            let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            let lifecycle_update = McpToolInvocationEventUpdate {
                state: settlement.state,
                dispatch_certainty: settlement.dispatch_certainty,
                outcome: Some(settlement.outcome),
                is_error: mcp_lifecycle_is_error(settlement.outcome),
                error_code: settlement.error_code.as_deref(),
                duration_ms: Some(elapsed_ms),
                output_truncated: settlement.output_truncated,
                result_size: settlement.result_size.clone(),
                failure_stage: settlement.failure_stage,
            };
            let lifecycle = mcp_tool_invocation_event(&approval, lifecycle_update.clone()).ok();
            let _ =
                self.settle_auto_mcp_action_journal(&journal, terminal_outcome, lifecycle.as_ref());
            if let Some(notifications) = context.notifications.as_ref() {
                emit_mcp_lifecycle_event(
                    notifications,
                    &context.run_id,
                    &approval,
                    lifecycle_update,
                );
            }
            return Ok(settlement.tool_result);
        }

        if self.claim_auto_mcp_dispatch(&mut journal).is_err() {
            self.invalidate_mcp_pending_payload(&action);
            let claim_failure = mcp_tool_invocation_event(
                &approval,
                McpToolInvocationEventUpdate {
                    state: AgentMcpToolInvocationState::Failed,
                    dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
                    outcome: Some(AgentMcpToolInvocationOutcome::TransportError),
                    is_error: Some(true),
                    error_code: Some("mcp.auto_dispatch_claim_failed"),
                    duration_ms: Some(0),
                    output_truncated: false,
                    result_size: None,
                    failure_stage: Some(AgentMcpInvocationFailureStage::Dispatch),
                },
            )
            .ok();
            let _ = self.settle_auto_mcp_action_journal(
                &journal,
                McpAutoActionJournalTerminalOutcome::Failed,
                claim_failure.as_ref(),
            );
            return Err(AgentError::structured(
                "mcp.auto_dispatch_claim_failed",
                "The MCP invocation lost its durable dispatch claim and was not sent.",
                serde_json::json!({
                    "type": "mcp_tool",
                    "code": "autoDispatchClaimFailed",
                    "retryable": false,
                    "dispatchCertainty": "definitely_not_dispatched",
                }),
            ));
        }

        if let Some(notifications) = context.notifications.as_ref() {
            emit_mcp_lifecycle_event(
                notifications,
                &context.run_id,
                &approval,
                McpToolInvocationEventUpdate {
                    state: AgentMcpToolInvocationState::Dispatching,
                    dispatch_certainty: AgentMcpDispatchCertainty::PossiblyDispatched,
                    outcome: None,
                    is_error: None,
                    error_code: None,
                    duration_ms: None,
                    output_truncated: false,
                    result_size: None,
                    failure_stage: None,
                },
            );
        }

        let invocation_result = invoker
            .invoke_approved(
                McpApprovedToolInvocation {
                    approval: approval.as_ref().clone(),
                },
                cancellation,
            )
            .await;
        // The payload is single-use. `invoke_approved` consumes it before dispatch; this delete
        // also closes every fail-before-consume path without creating a replayable payload.
        self.invalidate_mcp_pending_payload(&action);

        let mut settlement = settle_mcp_invocation(&approval, invocation_result, false);
        let journal_outcome = match settlement.state {
            AgentMcpToolInvocationState::Completed => {
                McpAutoActionJournalTerminalOutcome::Completed
            }
            AgentMcpToolInvocationState::OutcomeUnknown => {
                McpAutoActionJournalTerminalOutcome::OutcomeUnknown
            }
            _ => McpAutoActionJournalTerminalOutcome::Failed,
        };
        let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let lifecycle = mcp_tool_invocation_event(
            &approval,
            McpToolInvocationEventUpdate {
                state: settlement.state,
                dispatch_certainty: settlement.dispatch_certainty,
                outcome: Some(settlement.outcome),
                is_error: mcp_lifecycle_is_error(settlement.outcome),
                error_code: settlement.error_code.as_deref(),
                duration_ms: Some(elapsed_ms),
                output_truncated: settlement.output_truncated,
                result_size: settlement.result_size.clone(),
                failure_stage: settlement.failure_stage,
            },
        )
        .ok();
        if self
            .settle_auto_mcp_action_journal(&journal, journal_outcome, lifecycle.as_ref())
            .is_err()
        {
            // A durable terminal receipt could not be proven. Never expose the transient response
            // as authoritative: leave the `executing` row for startup reconciliation and report
            // outcome-unknown without retrying the external call.
            settlement = McpInvocationSettlement {
                tool_result: failed_mcp_tool_result(
                    &approval,
                    "mcp.tool_outcome_unknown",
                    "The MCP invocation may have completed, but its durable result boundary is unknown. Check the authoritative system before deciding whether to try again.",
                    AgentMcpDispatchCertainty::PossiblyDispatched,
                ),
                state: AgentMcpToolInvocationState::OutcomeUnknown,
                outcome: AgentMcpToolInvocationOutcome::OutcomeUnknown,
                error_code: Some("mcp.tool_outcome_unknown".to_string()),
                dispatch_certainty: AgentMcpDispatchCertainty::PossiblyDispatched,
                output_truncated: false,
                result_size: None,
                failure_stage: Some(AgentMcpInvocationFailureStage::Persistence),
            };
        }
        if let Some(notifications) = context.notifications.as_ref() {
            emit_mcp_lifecycle_event(
                notifications,
                &context.run_id,
                &approval,
                McpToolInvocationEventUpdate {
                    state: settlement.state,
                    dispatch_certainty: settlement.dispatch_certainty,
                    outcome: Some(settlement.outcome),
                    is_error: mcp_lifecycle_is_error(settlement.outcome),
                    error_code: settlement.error_code.as_deref(),
                    duration_ms: Some(elapsed_ms),
                    output_truncated: settlement.output_truncated,
                    result_size: settlement.result_size.clone(),
                    failure_stage: settlement.failure_stage,
                },
            );
        }
        Ok(settlement.tool_result)
    }

    pub(in crate::application::agent) async fn run_mcp_tool_execution(
        &self,
        mut record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
        dispatch_already_claimed: bool,
    ) {
        let run_id = record.snapshot.run_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        let AgentProposedAction::McpToolCall { approval } = record.snapshot.action.clone() else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
            return;
        };
        let Some(invoker) = self.mcp_tool_invoker.clone() else {
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
            return;
        };
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Cancelled);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Cancelled);
            return;
        }

        let cancellation = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation.clone());
        let guard_cancelled = guard
            .cancel_flag()
            .load(std::sync::atomic::Ordering::SeqCst);
        if guard_cancelled {
            cancellation.cancel();
        }

        let cancelled_before_dispatch = cancellation.is_cancelled();
        let preflight_result = if cancelled_before_dispatch {
            Err(AgentError::cancelled())
        } else {
            invoker.revalidate_approved(&approval)
        };
        if preflight_result.is_err() {
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
        }

        // This durable CAS is the conservative dispatch boundary. Startup recovery treats an
        // interrupted `executing` MCP action as outcome-unknown and never replays it. Catalog,
        // policy, expiry and authenticated-payload validation have already completed while the
        // call was definitely not dispatched; the bridge repeats them after this CAS as its
        // TOCTOU defense.
        if preflight_result.is_ok() && !dispatch_already_claimed {
            if self
                .transition_pending_status(&record, PendingActionStatus::Executing)
                .is_err()
            {
                self.invalidate_mcp_pending_payload(&record.snapshot.action);
                self.unregister_cancellation_if_current(&run_id, &cancellation);
                return;
            }
            record.snapshot.status = PendingActionStatus::Executing;
        }

        let _guard = guard;
        let started_at = Instant::now();
        if preflight_result.is_ok() {
            emit_mcp_lifecycle_event(
                &notifications,
                &run_id,
                &approval,
                McpToolInvocationEventUpdate {
                    state: AgentMcpToolInvocationState::Dispatching,
                    dispatch_certainty: AgentMcpDispatchCertainty::PossiblyDispatched,
                    outcome: None,
                    is_error: None,
                    error_code: None,
                    duration_ms: None,
                    output_truncated: false,
                    result_size: None,
                    failure_stage: None,
                },
            );
        }

        let failed_before_dispatch = preflight_result.is_err();
        let invocation_result = match preflight_result {
            Err(error) => Err(error),
            Ok(()) => {
                invoker
                    .invoke_approved(
                        McpApprovedToolInvocation {
                            approval: approval.as_ref().clone(),
                        },
                        cancellation.clone(),
                    )
                    .await
            }
        };
        // `invoke_approved` normally consumes the one-time payload before dispatch. A second
        // TOCTOU revalidation can fail before that consume, so every terminal return performs an
        // idempotent delete as a final cleanup boundary.
        self.invalidate_mcp_pending_payload(&record.snapshot.action);
        let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

        let settlement =
            settle_mcp_invocation(&approval, invocation_result, failed_before_dispatch);
        let tool_result = settlement.tool_result;
        let invocation_state = settlement.state;
        let invocation_outcome = settlement.outcome;
        let event_error_code = settlement.error_code;
        let dispatch_certainty = settlement.dispatch_certainty;
        let output_truncated = settlement.output_truncated;
        let result_size = settlement.result_size;
        let failure_stage = settlement.failure_stage;

        // The Provider ToolCall frozen in the checkpoint is immutable. Approval is represented by
        // the typed decision, invocation lifecycle and paired ToolResult; rewriting the committed
        // call from `required` to `approved` would violate the append-only trace contract.
        let mut agent_input = record.agent_input.clone();
        agent_input.approval_decision = Some(AgentApprovalDecision {
            action_id: record.snapshot.action_id.clone(),
            status: AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        agent_input.tool_continuation = Some(AgentToolContinuation {
            call: call.clone(),
            result: tool_result.clone(),
        });
        let mut persisted_agent_input = agent_input.clone();
        if let Some(continuation) = persisted_agent_input.tool_continuation.as_mut() {
            continuation.result = persisted_mcp_tool_result(&continuation.result);
        }
        let final_pending_status = if invocation_state == AgentMcpToolInvocationState::Completed {
            PendingActionStatus::Completed
        } else if invocation_state == AgentMcpToolInvocationState::Cancelled {
            PendingActionStatus::Cancelled
        } else {
            PendingActionStatus::Failed
        };
        let completed_at = now_ms();
        if self
            .commit_audited_result_trace_with_continuation(
                &record,
                &persisted_agent_input,
                final_pending_status,
                None,
                completed_at,
                &notifications,
            )
            .is_err()
        {
            // The invocation is never retried. Leaving the durable row in `executing` makes a
            // restart conservatively reconcile it as outcome-unknown.
            self.unregister_cancellation_if_current(&run_id, &cancellation);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                trace_sequence: None,
                message: "The MCP Tool result could not be durably recorded. Its external outcome must be treated as unknown until restart reconciliation completes.".to_string(),
                recoverable: false,
                code: Some("mcp_result_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }

        emit_mcp_lifecycle_event(
            &notifications,
            &run_id,
            &approval,
            McpToolInvocationEventUpdate {
                state: invocation_state,
                dispatch_certainty,
                outcome: Some(invocation_outcome),
                is_error: mcp_lifecycle_is_error(invocation_outcome),
                error_code: event_error_code.as_deref(),
                duration_ms: Some(elapsed_ms),
                output_truncated,
                result_size,
                failure_stage,
            },
        );
        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(cancellation.clone()),
        )
        .await;
        self.unregister_cancellation_if_current(&run_id, &cancellation);
    }

    pub(in crate::application::agent) fn queue_skill_script_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_skill_script_execution(execution_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(in crate::application::agent) fn queue_office_operation_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_office_operation_execution(execution_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(in crate::application::agent) async fn run_office_operation_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::OfficeOperation {
            mut office_operation,
        } = record.snapshot.action.clone()
        else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
            return;
        };
        office_operation.approval_status = AgentApprovalStatus::Approved;

        let mut file_effect_guard =
            match self.register_file_effect(&record.agent_input, &run_id, &record.storage_id) {
                Ok(guard) => guard,
                Err(_) => {
                    self.discard_usage_context(&run_id);
                    return;
                }
            };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        let run_cancellation_token = cancellation_token.clone();
        let cancel_flag = guard.cancel_flag();
        let service = self.clone();
        let agent_input = record.agent_input.clone();
        file_effect_guard.mark_effects_started();
        let task_result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            match service.restore_skill_resource_session(&agent_input) {
                Ok(skill_resources) => service.execute_office_operation(
                    &agent_input,
                    &office_operation,
                    skill_resources,
                    cancellation_token,
                    Some(cancel_flag),
                ),
                Err(error) => office_skill_resource_restore_failure(
                    &office_operation,
                    office_tool_name(office_operation.prepared.request.document_kind),
                    &error.to_string(),
                ),
            }
        })
        .await;
        let mut tool_result = match task_result {
            Ok(result) => result,
            Err(error) => AgentToolResult {
                exact_archive_file: None,
                call_id: action_id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation",
                    "code": "executionTaskFailed",
                    "recovery": "retry",
                })),
                error: Some(format!("Office operation task failed: {error}")),
            },
        };
        // The Office engine owns the commit boundary. A cancellation observed before its
        // commit-ready point produces a cancelled result and discards staging; once the atomic
        // publish begins, a later cancellation must not rewrite a completed commit as cancelled.
        let result_cancelled = tool_result
            .result
            .as_ref()
            .and_then(|result| result.get("cancelled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if result_cancelled {
            tool_result.ok = false;
            let message = "Office operation was cancelled.".to_string();
            tool_result.error = Some(message.clone());
            if let Some(result) = tool_result.result.as_mut().and_then(Value::as_object_mut) {
                result.insert("cancelled".to_string(), Value::Bool(true));
                result.insert(
                    "errorCode".to_string(),
                    Value::String("office.cancelled".to_string()),
                );
                result.insert("error".to_string(), Value::String(message));
            }
        }
        let desired_pending_status = if result_cancelled {
            PendingActionStatus::Cancelled
        } else if tool_result.ok {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        };
        let settlement = self.settle_manual_file_effect(
            &record,
            &call,
            desired_pending_status,
            tool_result,
            "office_operation",
            &notifications,
        );
        let (agent_input, tool_result, final_pending_status) = match settlement {
            ManualFileEffectSettlement::Committed {
                agent_input,
                tool_result,
                pending_status,
            } => (*agent_input, tool_result, pending_status),
            ManualFileEffectSettlement::CommittedAndAdvanced => {
                file_effect_guard.mark_durably_settled();
                self.unregister_cancellation(&run_id);
                return;
            }
            ManualFileEffectSettlement::Unsettled => {
                self.unregister_cancellation(&run_id);
                return;
            }
        };
        file_effect_guard.mark_durably_settled();
        drop(file_effect_guard);
        // The receipt is already durable, but a concurrent message/conversation deletion may now
        // own the scope. Do not emit a transient result for an owner that is being removed; the
        // continuation below observes the same marker and terminates without recreating state.
        {
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !deletion_lifecycle.contains_input(&record.agent_input) {
                let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                    run_id: run_id.clone(),
                    result: tool_result,
                }));
            }
        }
        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(run_cancellation_token),
        )
        .await;
    }

    pub(in crate::application::agent) async fn run_skill_script_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::SkillScript { mut script } = record.snapshot.action.clone() else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
            return;
        };
        script.approval_status = AgentApprovalStatus::Approved;

        let mut file_effect_guard =
            match self.register_file_effect(&record.agent_input, &run_id, &record.storage_id) {
                Ok(guard) => guard,
                Err(_) => {
                    self.discard_usage_context(&run_id);
                    return;
                }
            };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        let cancel_flag = guard.cancel_flag();
        let post_execution_cancel_flag = Arc::clone(&cancel_flag);
        let run_cancellation_token = cancellation_token.clone();
        let mut tool_result = match self.restore_skill_resource_session(&record.agent_input) {
            Err(error) => {
                drop(guard);
                AgentToolResult {
                    exact_archive_file: None,
                    call_id: action_id.clone(),
                    tool: "skills_run_script".to_string(),
                    ok: false,
                    result: Some(serde_json::json!({
                        "type": "skill_script",
                        "code": "snapshotUnavailable",
                        "recovery": "reactivateSkill",
                    })),
                    error: Some(error.to_string()),
                }
            }
            Ok(resources) => {
                let service = self.clone();
                let agent_input = record.agent_input.clone();
                file_effect_guard.mark_effects_started();
                match tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    service.execute_skill_script(
                        &agent_input,
                        &script,
                        resources.as_deref(),
                        CommandAuthorizationSource::ExplicitUser,
                        cancellation_token,
                        Some(cancel_flag),
                    )
                })
                .await
                {
                    Ok(result) => result,
                    Err(error) => AgentToolResult {
                        exact_archive_file: None,
                        call_id: action_id.clone(),
                        tool: "skills_run_script".to_string(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "type": "skill_script",
                            "code": "executionTaskFailed",
                            "recovery": "retry",
                        })),
                        error: Some(format!("Skill script execution task failed: {error}")),
                    },
                }
            }
        };
        let result_cancelled = run_cancellation_token.is_cancelled()
            || post_execution_cancel_flag.load(Ordering::SeqCst)
            || tool_result
                .result
                .as_ref()
                .and_then(|result| result.get("cancelled"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
        if result_cancelled {
            tool_result.ok = false;
            let message = "Skill script execution was cancelled.".to_string();
            tool_result.error = Some(message.clone());
            if let Some(result) = tool_result.result.as_mut().and_then(Value::as_object_mut) {
                result.insert("cancelled".to_string(), Value::Bool(true));
                result.insert(
                    "errorCode".to_string(),
                    Value::String("skill_script.cancelled".to_string()),
                );
                result.insert("error".to_string(), Value::String(message));
            }
        }
        let desired_pending_status = if result_cancelled {
            PendingActionStatus::Cancelled
        } else if tool_result.ok {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        };
        let settlement = self.settle_manual_file_effect(
            &record,
            &call,
            desired_pending_status,
            tool_result,
            "skill_script",
            &notifications,
        );
        let (agent_input, tool_result, final_pending_status) = match settlement {
            ManualFileEffectSettlement::Committed {
                agent_input,
                tool_result,
                pending_status,
            } => (*agent_input, tool_result, pending_status),
            ManualFileEffectSettlement::CommittedAndAdvanced => {
                file_effect_guard.mark_durably_settled();
                self.unregister_cancellation(&run_id);
                return;
            }
            ManualFileEffectSettlement::Unsettled => {
                self.unregister_cancellation(&run_id);
                return;
            }
        };
        file_effect_guard.mark_durably_settled();
        drop(file_effect_guard);
        {
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !deletion_lifecycle.contains_input(&record.agent_input) {
                let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                    run_id: run_id.clone(),
                    result: tool_result,
                }));
            }
        }
        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(run_cancellation_token),
        )
        .await;
    }

    pub(in crate::application::agent) async fn run_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::Command { command } = record.snapshot.action.clone() else {
            if let Err(error) =
                self.persist_pending_target_status(&record, PendingActionStatus::Failed)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Failed,
                    &error,
                );
                return;
            }
            if let Err(error) = self.transition_pending_status(&record, PendingActionStatus::Failed)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Failed,
                    &error,
                );
            }
            return;
        };

        let mut file_effect_guard =
            match self.register_file_effect(&record.agent_input, &run_id, &record.storage_id) {
                Ok(guard) => Some(guard),
                Err(_) => {
                    self.discard_usage_context(&run_id);
                    return;
                }
            };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation(&run_id);
            self.discard_usage_context(&run_id);
            return;
        }

        let cancel_flag = guard.cancel_flag();
        let post_execution_cancel_flag = Arc::clone(&cancel_flag);
        let run_cancellation_token = cancellation_token.clone();
        let command_for_error = command.clone();
        let artifact_runtime = self.artifact_runtime.clone();
        let office_engine = self.office_engine.clone();
        let skill_resources = self
            .restore_skill_resource_session(&record.agent_input)
            .ok()
            .flatten();
        let file_input_context = agent_file_input_execution_context(
            &record.agent_input,
            skill_resources,
            Arc::clone(&self.storage),
        );
        file_effect_guard
            .as_mut()
            .expect("approved command retains its file-effect lease before Session start")
            .mark_effects_started();
        let command_sessions = self.command_sessions.clone();
        let session_owner = CommandSessionOwner {
            conversation_id: record.snapshot.conversation_id.clone().unwrap_or_default(),
            assistant_message_id: record
                .snapshot
                .assistant_message_id
                .clone()
                .unwrap_or_default(),
            origin_run_id: run_id.clone(),
            call_id: command.id.clone(),
            project_id: agent_input_project_id(&record.agent_input).map(ToString::to_string),
        };
        let workspace_root = workspace_root_optional(&record.agent_input);
        let permissions = permissions_from_input(&record.agent_input);
        let session_notifications = notifications.clone();
        let cancellation_probe_token = cancellation_token.clone();
        let session_command = command.clone();
        let session_action_id = action_id.clone();
        let command_launch = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let mut file_effect_guard = file_effect_guard;
            let launch = command_sessions.start(StartAgentCommandSession {
                owner: session_owner,
                workspace_root: workspace_root.as_deref(),
                command: &session_command,
                permissions,
                authorization_source: CommandAuthorizationSource::ExplicitUser,
                approval_provenance: serde_json::json!({
                    "source": "explicit_user",
                    "actionId": session_action_id,
                    "approvalStatus": "approved",
                }),
                artifact_runtime,
                office_engine: Some(office_engine),
                file_inputs: Some(&file_input_context),
                notifications: Some(session_notifications),
                cancellation_token,
                cancel_probe: Some(Arc::new(move || {
                    cancellation_probe_token.is_cancelled() || cancel_flag.load(Ordering::SeqCst)
                })),
                file_effect_guard: &mut file_effect_guard,
            });
            (launch, file_effect_guard)
        })
        .await;

        let (command_launch, mut file_effect_guard) = match command_launch {
            Ok((launch, file_effect_guard)) => (launch, file_effect_guard),
            Err(error) => (Err(format!("命令执行任务失败：{error}")), None),
        };

        let mut command_result = match command_launch {
            Ok(AgentCommandSessionLaunch::Exited(terminal)) => terminal.execution,
            Ok(AgentCommandSessionLaunch::Running {
                snapshot,
                tool_result,
                mut handoff_guard,
            }) => {
                let mut continuation_input = record.agent_input.clone();
                continuation_input.approval_decision = Some(AgentApprovalDecision {
                    action_id: record.snapshot.action_id.clone(),
                    status: AgentApprovalDecisionStatus::Approved,
                    message: None,
                });
                continuation_input.tool_continuation = Some(AgentToolContinuation {
                    call: call.clone(),
                    result: tool_result.clone(),
                });
                let completed_at = now_ms();
                let mut settlement_errors = Vec::new();
                let mut handoff_already_advanced = false;
                let mut audit_definitely_uncommitted = false;
                let handoff = handoff_guard.commit(
                    || {
                        run_cancellation_token.is_cancelled()
                            || post_execution_cancel_flag.load(Ordering::SeqCst)
                    },
                    || {
                        for _ in 0..2 {
                            match self.commit_audited_result_trace_with_continuation(
                                &record,
                                &continuation_input,
                                PendingActionStatus::Completed,
                                None,
                                completed_at,
                                &notifications,
                            ) {
                                Ok(()) => return Ok(()),
                                Err(error) => settlement_errors.push(error),
                            }
                        }
                        match self.inspect_audited_result_trace_with_continuation(
                            &record,
                            &continuation_input,
                            PendingActionStatus::Completed,
                            None,
                            completed_at,
                        ) {
                            Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                                Ok(())
                            }
                            Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                                handoff_already_advanced = true;
                                Ok(())
                            }
                            Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {
                                audit_definitely_uncommitted = true;
                                Err(format!(
                                    "running receipt was definitely not committed: {}",
                                    settlement_errors.join("; retry: ")
                                ))
                            }
                            Ok(AgentPendingActionSettlementInspection::Diverged {
                                component,
                                reason,
                            }) => Err(format!(
                                "running receipt is divergent at {component}: {reason}; attempts: {}",
                                settlement_errors.join("; retry: ")
                            )),
                            Err(inspection_error) => Err(format!(
                                "running receipt could not be inspected: {inspection_error}; attempts: {}",
                                settlement_errors.join("; retry: ")
                            )),
                        }
                    },
                );

                match handoff {
                    Ok(AgentCommandHandoffOutcome::Adopted) => {
                        if handoff_already_advanced {
                            self.unregister_cancellation_if_current(
                                &run_id,
                                &run_cancellation_token,
                            );
                            return;
                        }
                        {
                            let deletion_lifecycle = self
                                .deletion_lifecycle
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            if !deletion_lifecycle.contains_input(&record.agent_input) {
                                let _ = notifications.send(agent_event_notification(
                                    AgentEvent::ToolResult {
                                        run_id: run_id.clone(),
                                        result: tool_result,
                                    },
                                ));
                            }
                        }
                        self.run_action_continuation(
                            record,
                            continuation_input,
                            notifications,
                            PendingActionStatus::Completed,
                            Some(run_cancellation_token),
                        )
                        .await;
                        return;
                    }
                    Ok(AgentCommandHandoffOutcome::CancelledBeforeCommit) => {
                        match handoff_guard.abort_before_handoff() {
                            Ok(terminal) => {
                                let mut execution = terminal.execution;
                                execution.cancelled = true;
                                execution
                            }
                            Err(error) => {
                                self.unregister_cancellation_if_current(
                                    &run_id,
                                    &run_cancellation_token,
                                );
                                let _ = notifications.send(agent_event_notification(
                                    AgentEvent::Error {
                                        run_id: Some(run_id),
                                        trace_sequence: None,
                                        message: "命令已在持久交接前取消，但无法确认进程已经终止；不会伪造终态回执。".to_string(),
                                        recoverable: true,
                                        code: Some(
                                            "command_session_pre_handoff_abort_failed".to_string(),
                                        ),
                                        details: Some(serde_json::json!({
                                            "type": "command_session",
                                            "code": "preHandoffTerminationUnconfirmed",
                                            "sessionId": snapshot.session_id,
                                            "effectsMayHaveOccurred": true,
                                            "terminationConfirmed": false,
                                            "terminationError": bounded_audit_error(&error),
                                        })),
                                    },
                                ));
                                return;
                            }
                        }
                    }
                    Ok(AgentCommandHandoffOutcome::PersistenceFailed(error))
                        if audit_definitely_uncommitted =>
                    {
                        match handoff_guard.abort_before_handoff() {
                            Ok(terminal) => {
                                let mut execution = terminal.execution;
                                execution.error.get_or_insert_with(|| {
                                    format!(
                                        "Command Session was terminated before handoff because its running receipt was not durable: {error}"
                                    )
                                });
                                execution
                            }
                            Err(abort_error) => {
                                self.unregister_cancellation_if_current(
                                    &run_id,
                                    &run_cancellation_token,
                                );
                                let _ = notifications.send(agent_event_notification(
                                    AgentEvent::Error {
                                        run_id: Some(run_id),
                                        trace_sequence: None,
                                        message: "命令的持久交接回执确认未提交，且无法确认进程已经终止；不会伪造已结算状态。".to_string(),
                                        recoverable: true,
                                        code: Some(
                                            "command_session_pre_handoff_abort_failed".to_string(),
                                        ),
                                        details: Some(serde_json::json!({
                                            "type": "command_session",
                                            "code": "preHandoffTerminationUnconfirmed",
                                            "sessionId": snapshot.session_id,
                                            "effectsMayHaveOccurred": true,
                                            "terminationConfirmed": false,
                                            "auditError": bounded_audit_error(&error),
                                            "terminationError": bounded_audit_error(&abort_error),
                                        })),
                                    },
                                ));
                                return;
                            }
                        }
                    }
                    Ok(AgentCommandHandoffOutcome::PersistenceFailed(error)) => {
                        let termination = handoff_guard.abort_before_handoff();
                        self.unregister_cancellation_if_current(&run_id, &run_cancellation_token);
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id),
                            trace_sequence: None,
                            message: "命令 Session 的持久交接结果无法权威确认；已停止续跑，且不会伪造已结算状态。".to_string(),
                            recoverable: true,
                            code: Some("command_session_handoff_indeterminate".to_string()),
                            details: Some(serde_json::json!({
                                "type": "command_session",
                                "code": "handoffIndeterminate",
                                "sessionId": snapshot.session_id,
                                "effectsMayHaveOccurred": true,
                                "terminationConfirmed": termination.is_ok(),
                                "auditError": bounded_audit_error(&error),
                                "terminationError": termination.as_ref().err().map(|value| bounded_audit_error(value)),
                                "execution": termination.as_ref().ok().map(|value| &value.execution),
                            })),
                        }));
                        return;
                    }
                    Err(error) => {
                        let termination = handoff_guard.abort_before_handoff();
                        self.unregister_cancellation_if_current(&run_id, &run_cancellation_token);
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id),
                            trace_sequence: None,
                            message: "命令 Session 所有权交接失败；已停止续跑。".to_string(),
                            recoverable: true,
                            code: Some("command_session_handoff_failed".to_string()),
                            details: Some(serde_json::json!({
                                "type": "command_session",
                                "code": "handoffFailed",
                                "sessionId": snapshot.session_id,
                                "effectsMayHaveOccurred": true,
                                "terminationConfirmed": termination.is_ok(),
                                "handoffError": bounded_audit_error(&error),
                                "terminationError": termination.as_ref().err().map(|value| bounded_audit_error(value)),
                            })),
                        }));
                        return;
                    }
                }
            }
            Err(error) => failed_command_result(&command_for_error, error, None),
        };
        let execution_was_cancelled = run_cancellation_token.is_cancelled()
            || post_execution_cancel_flag.load(Ordering::SeqCst)
            || command_result.cancelled;
        if execution_was_cancelled {
            command_result.cancelled = true;
        }

        let (agent_input, final_pending_status) = {
            let command_succeeded = !execution_was_cancelled
                && command_result.error.is_none()
                && !command_result.timed_out
                && !command_result.cancelled
                && command_result.exit_code == Some(0);
            let desired_pending_status = if execution_was_cancelled {
                PendingActionStatus::Cancelled
            } else if command_succeeded {
                PendingActionStatus::Completed
            } else {
                PendingActionStatus::Failed
            };
            let mut final_pending_status = desired_pending_status;
            let mut tool_result = command_tool_result(&action_id, &command_result);
            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: action_id.clone(),
                status: AgentApprovalDecisionStatus::Approved,
                message: None,
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call: call.clone(),
                result: tool_result.clone(),
            });

            // Capture one immutable terminal timestamp and retry the exact settlement. The
            // storage transaction is idempotent, so a SQLite commit that succeeded but returned
            // an error is recovered without replacing the real result with a synthetic failure.
            let completed_at = now_ms();
            let mut settlement_errors = Vec::new();
            let mut settled = false;
            for _ in 0..2 {
                match self.commit_audited_command_result_trace_with_continuation(
                    &record,
                    &agent_input,
                    desired_pending_status,
                    &command_result,
                    completed_at,
                    &notifications,
                ) {
                    Ok(()) => {
                        settled = true;
                        break;
                    }
                    Err(error) => settlement_errors.push(error),
                }
            }

            if !settled {
                match self.inspect_audited_command_result_trace_with_continuation(
                    &record,
                    &agent_input,
                    desired_pending_status,
                    &command_result,
                    completed_at,
                ) {
                    Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                        settled = true;
                    }
                    Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                        if let Some(guard) = file_effect_guard.as_mut() {
                            guard.mark_durably_settled();
                        }
                        self.unregister_cancellation(&run_id);
                        return;
                    }
                    Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {}
                    Ok(AgentPendingActionSettlementInspection::Diverged { component, reason }) => {
                        self.unregister_cancellation(&run_id);
                        emit_manual_command_settlement_error(
                            &notifications,
                            &run_id,
                            ManualCommandSettlementError {
                                code: "approval_result_commit_indeterminate",
                                message: format!(
                                    "命令已经结束，但审计事务处于冲突或部分提交状态；已停止续跑：{component}: {reason}"
                                ),
                                attempt_error: &settlement_errors.join("; retry: "),
                                inspection_error: Some(&reason),
                                command_result: &command_result,
                                tool_result: &tool_result,
                            },
                        );
                        return;
                    }
                    Err(inspection_error) => {
                        self.unregister_cancellation(&run_id);
                        emit_manual_command_settlement_error(
                            &notifications,
                            &run_id,
                            ManualCommandSettlementError {
                                code: "approval_result_commit_indeterminate",
                                message: format!(
                                    "命令已经结束，但无法权威核对审计事务是否提交；已停止续跑：{inspection_error}"
                                ),
                                attempt_error: &settlement_errors.join("; retry: "),
                                inspection_error: Some(&inspection_error),
                                command_result: &command_result,
                                tool_result: &tool_result,
                            },
                        );
                        return;
                    }
                }
            }

            if !settled {
                let audit_error = settlement_errors.join("; retry: ");
                tool_result = command_audit_persistence_failure(
                    &command_for_error,
                    "afterExecution",
                    &audit_error,
                    Some(&command_result),
                );
                final_pending_status = PendingActionStatus::Failed;
                agent_input.tool_continuation = Some(AgentToolContinuation {
                    call: call.clone(),
                    result: tool_result.clone(),
                });

                let mut failure_errors = Vec::new();
                for _ in 0..2 {
                    match self.commit_audited_command_result_trace_with_continuation(
                        &record,
                        &agent_input,
                        final_pending_status,
                        &command_result,
                        completed_at,
                        &notifications,
                    ) {
                        Ok(()) => {
                            settled = true;
                            break;
                        }
                        Err(error) => failure_errors.push(error),
                    }
                }

                if !settled {
                    let persistence_error = failure_errors.join("; retry: ");
                    match self.inspect_audited_command_result_trace_with_continuation(
                        &record,
                        &agent_input,
                        final_pending_status,
                        &command_result,
                        completed_at,
                    ) {
                        Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                            settled = true;
                        }
                        Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                            if let Some(guard) = file_effect_guard.as_mut() {
                                guard.mark_durably_settled();
                            }
                            self.unregister_cancellation(&run_id);
                            return;
                        }
                        Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {
                            self.unregister_cancellation(&run_id);
                            emit_manual_command_settlement_error(
                                &notifications,
                                &run_id,
                                ManualCommandSettlementError {
                                    code: "approval_result_persistence_failed",
                                    message: format!(
                                        "命令已经结束，但审计、目标终态和工具结果轨迹确认未提交；已停止续跑：{persistence_error}"
                                    ),
                                    attempt_error: &persistence_error,
                                    inspection_error: None,
                                    command_result: &command_result,
                                    tool_result: &tool_result,
                                },
                            );
                            return;
                        }
                        Ok(AgentPendingActionSettlementInspection::Diverged {
                            component,
                            reason,
                        }) => {
                            self.unregister_cancellation(&run_id);
                            emit_manual_command_settlement_error(
                                &notifications,
                                &run_id,
                                ManualCommandSettlementError {
                                    code: "approval_result_commit_indeterminate",
                                    message: format!(
                                        "命令已经结束，但失败回执处于冲突或部分提交状态；已停止续跑：{component}: {reason}"
                                    ),
                                    attempt_error: &persistence_error,
                                    inspection_error: Some(&reason),
                                    command_result: &command_result,
                                    tool_result: &tool_result,
                                },
                            );
                            return;
                        }
                        Err(inspection_error) => {
                            self.unregister_cancellation(&run_id);
                            emit_manual_command_settlement_error(
                                &notifications,
                                &run_id,
                                ManualCommandSettlementError {
                                    code: "approval_result_commit_indeterminate",
                                    message: format!(
                                        "命令已经结束，但无法权威核对失败回执是否提交；已停止续跑：{inspection_error}"
                                    ),
                                    attempt_error: &persistence_error,
                                    inspection_error: Some(&inspection_error),
                                    command_result: &command_result,
                                    tool_result: &tool_result,
                                },
                            );
                            return;
                        }
                    }
                }
            }
            debug_assert!(settled);
            if let Some(guard) = file_effect_guard.as_mut() {
                guard.mark_durably_settled();
            }
            (agent_input, final_pending_status)
        };
        // The file-producing boundary is now durably paired with its ToolResult. Release the
        // project effect lease before model continuation; a concurrent deletion may proceed and
        // the continuation's deletion marker check will then stop any new action.
        drop(file_effect_guard);

        if execution_was_cancelled && final_pending_status == PendingActionStatus::Cancelled {
            const REASON: &str =
                "Agent run was cancelled while the approved command was executing.";
            if self.is_agent_input_scope_deleting(&record.agent_input) {
                self.discard_usage_context(&run_id);
                if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                    self.release_conversation_turn_if_current(conversation_id, &run_id);
                }
                self.release_turn_concurrency_permit(&run_id);
                self.discard_trace_snapshot(&run_id);
                self.discard_exact_running_context_window_snapshot(&run_id);
                self.unregister_cancellation(&run_id);
                return;
            }
            if let Some(continuation) = agent_input.tool_continuation.as_ref() {
                let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                    run_id: run_id.clone(),
                    result: continuation.result.clone(),
                }));
            }
            let continuation_snapshot = self
                .trace_snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&run_id)
                .cloned();
            let terminal =
                if let (Some(conversation_id), Some(assistant_message_id), Some(snapshot)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                    continuation_snapshot,
                ) {
                    // Settlement already committed the exact Archive pointer and the central-gated
                    // model observation into this authoritative snapshot.
                    let terminal = cancelled_conversation_trace_from_snapshot(
                        snapshot,
                        &run_id,
                        conversation_id,
                        assistant_message_id,
                        REASON,
                    );
                    terminal.map(|terminal| {
                        (
                            conversation_id.to_string(),
                            assistant_message_id.to_string(),
                            terminal,
                        )
                    })
                } else {
                    Err(
                        "cancelled command is missing its settled conversation trace snapshot"
                            .to_string(),
                    )
                };
            let mut output = AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Cancelled,
                run_id: run_id.clone(),
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: Some(REASON.to_string()),
                proposed_actions: Vec::new(),
                conversation_turn_trace: terminal
                    .as_ref()
                    .ok()
                    .map(|(_, _, terminal)| terminal.trace.clone()),
            };
            let previous_usage_state = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&run_id)
                .cloned();
            let persisted = persist_terminal_with_bounded_retry(
                || {
                    let deletion_lifecycle = self
                        .deletion_lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if deletion_lifecycle.contains_input(&record.agent_input) {
                        return Err("项目或会话正在移除，无法持久化已取消命令终态。".to_string());
                    }
                    terminal.as_ref().map_err(Clone::clone).and_then(
                        |(conversation_id, assistant_message_id, terminal)| {
                            self.persist_final_assistant_output_with_model_context(
                                conversation_id,
                                assistant_message_id,
                                &mut output,
                                &terminal.model_context_items,
                            )
                        },
                    )
                },
                || restore_run_usage_state(self, &run_id, &previous_usage_state),
            )
            .await;
            let pending_transition = if persisted.is_ok() {
                persist_terminal_with_bounded_retry(
                    || {
                        let deletion_lifecycle = self
                            .deletion_lifecycle
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if deletion_lifecycle.contains_input(&record.agent_input) {
                            return Err(
                                "项目或会话正在移除，无法完成已取消命令状态迁移。".to_string()
                            );
                        }
                        self.transition_pending_status(&record, PendingActionStatus::Cancelled)
                    },
                    || {},
                )
                .await
            } else {
                Err("已取消命令终态未持久化，已保留 pending 记录供启动对账。".to_string())
            };
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if deletion_lifecycle.contains_input(&record.agent_input) {
                drop(deletion_lifecycle);
                self.discard_usage_context(&run_id);
                if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                    self.release_conversation_turn_if_current(conversation_id, &run_id);
                }
                self.release_turn_concurrency_permit(&run_id);
                self.discard_trace_snapshot(&run_id);
                self.discard_exact_running_context_window_snapshot(&run_id);
                self.unregister_cancellation(&run_id);
                return;
            }
            if let Err(error) = persisted {
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id.clone()),
                    trace_sequence: None,
                    message: format!(
                        "无法原子持久化已取消命令的 assistant 终态与会话轨迹：{error}"
                    ),
                    recoverable: true,
                    code: Some("conversation_trace_persistence_failed".to_string()),
                    details: None,
                }));
                return;
            }
            if let Err(error) = pending_transition {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Cancelled,
                    &error,
                );
                return;
            }
            if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                self.release_conversation_turn_if_current(conversation_id, &run_id);
            }
            self.release_turn_concurrency_permit(&run_id);
            if let Some(assistant_message_id) = record.snapshot.assistant_message_id.as_deref() {
                self.notify_durable_turn_observers(assistant_message_id);
            }
            self.discard_trace_snapshot(&run_id);
            self.discard_exact_running_context_window_snapshot(&run_id);
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                run_id,
                success: false,
                status: Some(AgentRunStatus::Cancelled),
                content: None,
                usage: output.usage,
                finish_reason: Some(REASON.to_string()),
                proposed_actions: Vec::new(),
            }));
            return;
        }

        {
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !deletion_lifecycle.contains_input(&record.agent_input) {
                if let Some(continuation) = agent_input.tool_continuation.as_ref() {
                    let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                        run_id: run_id.clone(),
                        result: continuation.result.clone(),
                    }));
                }
            }
        }

        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(run_cancellation_token),
        )
        .await;
    }

    async fn finish_cancelled_action_continuation(
        &self,
        record: &PendingActionRecord,
        notifications: &CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
        cancellation_token: &AgentCancellationToken,
    ) {
        const REASON: &str =
            "Agent run was cancelled after the approved tool result was durably recorded.";
        const PERSISTENCE_ERROR: &str =
            "The cancelled Agent run could not be durably finalized. It will be reconciled on restart.";
        let run_id = &record.snapshot.run_id;

        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(run_id);
            if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                self.release_conversation_turn_if_current(conversation_id, run_id);
            }
            self.release_turn_concurrency_permit(run_id);
            self.discard_trace_snapshot(run_id);
            self.discard_exact_running_context_window_snapshot(run_id);
            self.unregister_cancellation_if_current(run_id, cancellation_token);
            return;
        }

        let snapshot = self
            .trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned();
        let terminal = match (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
            snapshot,
        ) {
            (Some(conversation_id), Some(assistant_message_id), Some(snapshot)) => {
                cancelled_conversation_trace_from_snapshot(
                    snapshot,
                    run_id,
                    conversation_id,
                    assistant_message_id,
                    REASON,
                )
                .map(|terminal| {
                    (
                        conversation_id.to_string(),
                        assistant_message_id.to_string(),
                        terminal,
                    )
                })
            }
            _ => Err(
                "cancelled action continuation is missing its durable conversation trace snapshot"
                    .to_string(),
            ),
        };
        let mut output = AgentChatOutput {
            content: String::new(),
            status: AgentRunStatus::Cancelled,
            run_id: run_id.clone(),
            events: Vec::new(),
            tool_definitions: Vec::new(),
            todo: None,
            usage: None,
            finish_reason: Some(REASON.to_string()),
            proposed_actions: Vec::new(),
            conversation_turn_trace: terminal
                .as_ref()
                .ok()
                .map(|(_, _, terminal)| terminal.trace.clone()),
        };
        let previous_usage_state = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned();
        let persisted = persist_terminal_with_bounded_retry(
            || {
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&record.agent_input) {
                    return Err("项目或会话正在移除，无法持久化已取消审批续跑终态。".to_string());
                }
                terminal.as_ref().map_err(Clone::clone).and_then(
                    |(conversation_id, assistant_message_id, terminal)| {
                        self.persist_final_assistant_output_with_model_context(
                            conversation_id,
                            assistant_message_id,
                            &mut output,
                            &terminal.model_context_items,
                        )
                    },
                )
            },
            || restore_run_usage_state(self, run_id, &previous_usage_state),
        )
        .await;
        let pending_transition = if persisted.is_ok() {
            persist_terminal_with_bounded_retry(
                || {
                    let deletion_lifecycle = self
                        .deletion_lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if deletion_lifecycle.contains_input(&record.agent_input) {
                        return Err(
                            "项目或会话正在移除，无法完成已取消审批续跑的状态迁移。".to_string()
                        );
                    }
                    self.transition_pending_status(record, final_pending_status)
                },
                || {},
            )
            .await
        } else {
            Err(
                "已取消审批续跑的 assistant 终态未持久化，已保留非终态 pending 记录供启动对账。"
                    .to_string(),
            )
        };
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deletion_lifecycle.contains_input(&record.agent_input) {
            drop(deletion_lifecycle);
            self.discard_usage_context(run_id);
            if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                self.release_conversation_turn_if_current(conversation_id, run_id);
            }
            self.release_turn_concurrency_permit(run_id);
            self.discard_trace_snapshot(run_id);
            self.discard_exact_running_context_window_snapshot(run_id);
            self.unregister_cancellation_if_current(run_id, cancellation_token);
            return;
        }
        if let Err(error) = persisted {
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id.clone()),
                trace_sequence: None,
                message: PERSISTENCE_ERROR.to_string(),
                recoverable: false,
                code: Some("cancelled_run_persistence_failed".to_string()),
                details: None,
            }));
            eprintln!("failed to persist cancelled action continuation: {error}");
            return;
        }
        if let Err(error) = pending_transition {
            emit_pending_transition_error(notifications, run_id, final_pending_status, &error);
            return;
        }

        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.invalidate_conversation_context_state(conversation_id);
        }
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.release_conversation_turn_if_current(conversation_id, run_id);
        }
        self.release_turn_concurrency_permit(run_id);
        if let Some(assistant_message_id) = record.snapshot.assistant_message_id.as_deref() {
            self.notify_durable_turn_observers(assistant_message_id);
        }
        self.discard_trace_snapshot(run_id);
        self.discard_exact_running_context_window_snapshot(run_id);
        self.unregister_cancellation_if_current(run_id, cancellation_token);
        let _ = notifications.send(agent_event_notification(AgentEvent::Done {
            run_id: run_id.clone(),
            success: false,
            status: Some(AgentRunStatus::Cancelled),
            content: None,
            usage: output.usage,
            finish_reason: output.finish_reason,
            proposed_actions: Vec::new(),
        }));
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_pre_runtime_action_continuation_failure(
        &self,
        record: &PendingActionRecord,
        notifications: &CoreServerNotificationSender,
        steer_input: &AgentSteerInputQueue,
        cancellation_token: &AgentCancellationToken,
        expected_target_status: PendingActionStatus,
        failure_code: &str,
        failure_message: String,
    ) {
        let run_id = &record.snapshot.run_id;
        let (Some(conversation_id), Some(assistant_message_id)) = (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
        ) else {
            self.discard_usage_context(run_id);
            let _ = self.unregister_active_run_control(
                run_id,
                steer_input,
                AgentSteerRunRejectionCode::RunNotSteerable,
                "The agent run has finished and no longer accepts guidance.",
                notifications,
            );
            self.unregister_cancellation_if_current(run_id, cancellation_token);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id.clone()),
                trace_sequence: None,
                message: "审批续跑缺少 Conversation Turn 持久化身份。".to_string(),
                recoverable: true,
                code: Some("conversation_turn_identity_missing".to_string()),
                details: None,
            }));
            return;
        };

        let steering_close_error = self
            .unregister_active_run_control(
                run_id,
                steer_input,
                AgentSteerRunRejectionCode::RunNotSteerable,
                "The agent run has finished and no longer accepts guidance.",
                notifications,
            )
            .err();
        self.unregister_cancellation_if_current(run_id, cancellation_token);
        let terminal_message = match steering_close_error {
            Some(error) => {
                format!("{failure_message}；同时无法关闭审批续跑的用户引导通道：{error}")
            }
            None => failure_message,
        };

        let terminal_projection = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)
            .and_then(|trace| match trace {
                Some(trace)
                    if trace.run_id == *run_id
                        && trace.conversation_id == conversation_id
                        && trace.terminal_status
                            == ConversationTurnTraceTerminalStatus::InProgress =>
                {
                    let model_context_items = self
                        .storage
                        .get_conversation_model_context_log(assistant_message_id)?
                        .map(|log| log.items)
                        .unwrap_or_default();
                    let next_sequence = trace
                        .items
                        .last()
                        .map(ConversationTurnTraceItem::sequence)
                        .unwrap_or(0)
                        .saturating_add(1);
                    terminal_conversation_trace_from_snapshot(
                        ConversationTraceSnapshot {
                            items: trace.items,
                            model_context_items,
                            next_sequence,
                            truncated: trace.truncated,
                        },
                        run_id,
                        conversation_id,
                        assistant_message_id,
                        ConversationTurnTraceTerminalStatus::Failed,
                        &terminal_message,
                    )
                }
                Some(_) => {
                    Err("审批续跑的 durable ConversationTurnTrace 身份或状态不一致。".to_string())
                }
                None => Err("审批续跑的 durable ConversationTurnTrace 不存在。".to_string()),
            });
        let terminal = match terminal_projection {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id.clone()),
                    trace_sequence: None,
                    message: format!("无法构造审批续跑的失败终态：{error}"),
                    recoverable: true,
                    code: Some("conversation_trace_persistence_failed".to_string()),
                    details: None,
                }));
                return;
            }
        };
        let previous_usage_state = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned();
        let usage_record = self.prepare_run_usage_record(
            run_id,
            AgentRunStatus::Failed,
            None,
            Some(terminal_message.clone()),
        );
        let cumulative_usage = self.preview_cumulative_run_usage(run_id, None);
        let completed_at = now_ms();
        let persisted = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let pending = pending_actions
                .get_mut(&record.storage_id)
                .ok_or_else(|| format!("审批续跑的内存 pending 状态不存在：{}", record.storage_id));
            pending.and_then(|pending| {
                if pending.snapshot.run_id != *run_id
                    || pending.snapshot.conversation_id.as_deref() != Some(conversation_id)
                    || pending.snapshot.assistant_message_id.as_deref()
                        != Some(assistant_message_id)
                {
                    return Err("审批续跑的内存 pending 身份不一致。".to_string());
                }
                let expected_status = pending.snapshot.status;
                self.storage.fail_claimed_agent_action_continuation(
                    &record.storage_id,
                    pending_status_label(expected_status),
                    pending_status_label(expected_target_status),
                    conversation_id,
                    assistant_message_id,
                    &terminal_message,
                    &terminal.trace,
                    &terminal.model_context_items,
                    completed_at,
                    usage_record.as_ref(),
                )?;
                pending.snapshot.status = PendingActionStatus::Failed;
                Ok(())
            })
        };
        if let Err(error) = persisted {
            let mut usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner());
            match previous_usage_state {
                Some(previous) => {
                    usage_contexts.insert(run_id.clone(), previous);
                }
                None => {
                    usage_contexts.remove(run_id);
                }
            }
            drop(usage_contexts);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id.clone()),
                trace_sequence: None,
                message: format!(
                    "无法原子持久化 pending、assistant、轨迹与 Usage 的失败终态：{error}"
                ),
                recoverable: true,
                code: Some("conversation_trace_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }
        self.finish_persisted_run_usage(run_id, AgentRunStatus::Failed);
        self.notify_durable_turn_observers(assistant_message_id);

        self.emit_terminal_context_window_snapshot(
            notifications,
            &record.agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            &terminal_message,
        );
        self.release_conversation_turn_if_current(conversation_id, run_id);
        self.release_turn_concurrency_permit(run_id);
        self.discard_trace_snapshot(run_id);
        self.discard_exact_running_context_window_snapshot(run_id);
        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
            run_id: Some(run_id.clone()),
            trace_sequence: None,
            message: terminal_message.clone(),
            recoverable: false,
            code: Some(failure_code.to_string()),
            details: None,
        }));
        let _ = notifications.send(agent_event_notification(AgentEvent::Done {
            run_id: run_id.clone(),
            success: false,
            status: Some(AgentRunStatus::Failed),
            content: Some(terminal_message),
            usage: cumulative_usage,
            finish_reason: None,
            proposed_actions: Vec::new(),
        }));
    }

    pub(in crate::application::agent) async fn run_action_continuation(
        &self,
        record: PendingActionRecord,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
        existing_cancellation_token: Option<AgentCancellationToken>,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let cancellation_token = existing_cancellation_token.unwrap_or_default();
        let frozen_identity = record
            .agent_input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.as_ref());
        let resumed_identity = agent_input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.clone());
        if frozen_identity != resumed_identity.as_ref() {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                trace_sequence: None,
                message: "审批续跑的 Collaboration identity 与冻结 checkpoint 不一致。".to_string(),
                recoverable: true,
                code: Some("collaboration_identity_mismatch".to_string()),
                details: None,
            }));
            return;
        }
        let active_child_wake = if let Some(identity) = resumed_identity.as_ref() {
            match ChildAgentFactory::new(Arc::clone(&self.storage))
                .resolve_trusted_active_wake_by_identity(identity)
            {
                Ok(bundle) => Some(bundle),
                Err(error) => {
                    self.discard_usage_context(&run_id);
                    self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                        run_id: Some(run_id),
                        trace_sequence: None,
                        message: format!(
                            "子 Agent 审批续跑的 Host 协作身份已失效，已拒绝执行：{error}"
                        ),
                        recoverable: true,
                        code: Some("collaboration_identity_revalidation_failed".to_string()),
                        details: None,
                    }));
                    return;
                }
            }
        } else {
            None
        };
        if cancellation_token.is_cancelled() {
            self.finish_cancelled_action_continuation(
                &record,
                &notifications,
                final_pending_status,
                &cancellation_token,
            )
            .await;
            return;
        }
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            return;
        }
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            self.discard_usage_context(&run_id);
            return;
        }
        let (turn_conversation_id, turn_assistant_message_id) = match (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
        ) {
            (Some(conversation_id), Some(assistant_message_id)) => (
                conversation_id.to_string(),
                assistant_message_id.to_string(),
            ),
            _ => {
                self.discard_usage_context(&run_id);
                self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id),
                    trace_sequence: None,
                    message: "审批续跑缺少 Conversation Turn 持久化身份。".to_string(),
                    recoverable: true,
                    code: Some("conversation_turn_identity_missing".to_string()),
                    details: None,
                }));
                return;
            }
        };
        if let Err(error) = self.ensure_conversation_turn_owner(
            &turn_conversation_id,
            &run_id,
            &turn_assistant_message_id,
        ) {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                trace_sequence: None,
                message: format!("审批续跑无法取得 Conversation Turn：{error}"),
                recoverable: true,
                code: Some("conversation_turn_ownership_conflict".to_string()),
                details: None,
            }));
            return;
        }
        if let Err(error) = self.ensure_turn_concurrency_permit(&run_id) {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                trace_sequence: None,
                message: format!("审批续跑暂时无法取得进程级 Agent Turn 并发许可：{error}"),
                recoverable: true,
                code: Some("agent_turn_concurrency_limit".to_string()),
                details: None,
            }));
            return;
        }
        let steer_input = self.register_active_run_control(
            &run_id,
            &turn_conversation_id,
            &turn_assistant_message_id,
            record
                .agent_input
                .context
                .as_ref()
                .and_then(|context| context.project_id.as_deref()),
            record.agent_input.model_capabilities,
        );

        let skill_resources = match self.restore_skill_resource_session(&agent_input) {
            Ok(resources) => resources,
            Err(error) => {
                self.finish_pre_runtime_action_continuation_failure(
                    &record,
                    &notifications,
                    &steer_input,
                    &cancellation_token,
                    final_pending_status,
                    "skill_resource_snapshot_unavailable",
                    error.to_string(),
                );
                return;
            }
        };
        let mcp_tools = self.capture_mcp_tool_runtime(&record.agent_input);
        let initial_context_window_tool_projection = match self
            .context_window_tool_projection_with_mcp(
                &record.agent_input,
                skill_resources.clone(),
                mcp_tools.clone(),
            ) {
            Ok(projection) => projection,
            Err(error) => {
                self.finish_pre_runtime_action_continuation_failure(
                    &record,
                    &notifications,
                    &steer_input,
                    &cancellation_token,
                    final_pending_status,
                    "context_window_tool_projection_unavailable",
                    error,
                );
                return;
            }
        };
        let context_window_tool_projection =
            RunContextToolProjection::new(initial_context_window_tool_projection);
        if let Some(active) = active_child_wake.as_ref() {
            if let Err(error) = self.storage.transition_agent_wake(
                &active.spawn.initial_wake.wake_id,
                mycopilot_core::AgentWakeStatus::WaitingForApproval,
                mycopilot_core::AgentWakeStatus::Running,
                Some(&active.claim_token),
            ) {
                self.finish_pre_runtime_action_continuation_failure(
                    &record,
                    &notifications,
                    &steer_input,
                    &cancellation_token,
                    final_pending_status,
                    "collaboration_wake_resume_transition_failed",
                    format!("子 Agent Wake 无法持久进入续跑状态：{error}"),
                );
                return;
            }
            self.notify_durable_turn_observers(&turn_assistant_message_id);
        }
        let RuntimeTurnSegmentOutcome {
            result,
            terminal_event_gate,
        } = self
            .run_prepared_turn_segment(
                PreparedRuntimeTurnSegment {
                    run_id: run_id.clone(),
                    conversation_id: turn_conversation_id.clone(),
                    assistant_message_id: turn_assistant_message_id.clone(),
                    assistant_created_at: record.snapshot.created_at,
                    agent_input,
                    skill_resources,
                    mcp_tools,
                    context_window_tool_projection,
                    cancellation_token: cancellation_token.clone(),
                    steer_input,
                    pending_action_predecessor_settlement: Some((
                        record.clone(),
                        final_pending_status,
                    )),
                    invalidate_mcp_payload_on_pending_store_failure: false,
                    steering_close_error_context: "无法关闭审批续跑的用户引导通道并持久化剩余引导",
                },
                notifications.clone(),
            )
            .await;
        let keep_trace_snapshot = matches!(
            &result,
            Ok(output) if output.status == AgentRunStatus::WaitingForApproval
        );

        if self.is_agent_input_scope_deleting(&record.agent_input) {
            terminal_event_gate.discard();
            self.release_conversation_turn_if_current(&turn_conversation_id, &run_id);
            self.release_turn_concurrency_permit(&run_id);
            self.discard_usage_context(&run_id);
            self.discard_trace_snapshot(&run_id);
            self.discard_exact_running_context_window_snapshot(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            return;
        }

        let mut deletion_cleanup = false;
        let (durable_turn_terminal, settlement_committed) = match result {
            Ok(mut agent_output) => {
                let committed_durable_context = is_terminal_run_status(agent_output.status);
                let owner_ids = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                );
                let previous_usage_state = self
                    .usage_contexts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&run_id)
                    .cloned();
                let persisted = persist_terminal_with_bounded_retry(
                    || {
                        let deletion_lifecycle = self
                            .deletion_lifecycle
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if deletion_lifecycle.contains_input(&record.agent_input) {
                            return Err("项目或会话正在移除，无法持久化审批续跑终态。".to_string());
                        }
                        match owner_ids {
                            (Some(conversation_id), Some(assistant_message_id)) => self
                                .persist_final_assistant_output(
                                    conversation_id,
                                    assistant_message_id,
                                    &mut agent_output,
                                ),
                            _ => Err("审批续跑缺少 assistant 持久化身份。".to_string()),
                        }
                    },
                    || restore_run_usage_state(self, &run_id, &previous_usage_state),
                )
                .await;
                let pending_transition = if persisted.is_ok() {
                    persist_terminal_with_bounded_retry(
                        || {
                            let deletion_lifecycle = self
                                .deletion_lifecycle
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            if deletion_lifecycle.contains_input(&record.agent_input) {
                                return Err(
                                    "项目或会话正在移除，无法完成审批状态迁移。".to_string()
                                );
                            }
                            self.transition_pending_status(&record, final_pending_status)
                        },
                        || {},
                    )
                    .await
                } else {
                    Err("assistant 终态未持久化，已保留非终态 pending 记录供启动对账。".to_string())
                };
                let settlement_committed = persisted.is_ok() && pending_transition.is_ok();
                if let Err(error) = &pending_transition {
                    terminal_event_gate.discard();
                    emit_pending_transition_error(
                        &notifications,
                        &run_id,
                        final_pending_status,
                        error,
                    );
                }
                let terminal_commit_published = pending_terminal_commit_is_publishable(
                    persisted.is_ok(),
                    pending_transition.is_ok(),
                    committed_durable_context,
                );
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&record.agent_input) {
                    deletion_cleanup = true;
                    terminal_event_gate.discard();
                    self.discard_usage_context(&run_id);
                } else if let (Some(conversation_id), Some(assistant_message_id)) = owner_ids {
                    if terminal_commit_published {
                        self.notify_durable_turn_observers(assistant_message_id);
                        self.emit_terminal_context_window_snapshot(
                            &notifications,
                            &record.agent_input,
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            if agent_output.status == AgentRunStatus::Cancelled {
                                ""
                            } else {
                                &agent_output.content
                            },
                        );
                    } else if let Err(error) = &persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            trace_sequence: None,
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if terminal_commit_published {
                        emit_terminal_events_after_persistence_for_turn(
                            &notifications,
                            &terminal_event_gate,
                            &agent_output,
                            resumed_identity.as_ref(),
                            assistant_message_id,
                        );
                    }
                }
                (
                    terminal_commit_published && !deletion_cleanup,
                    settlement_committed,
                )
            }
            Err(error) => {
                terminal_event_gate.discard();
                let model_request_interruption = error.model_request_interruption();
                let usage = error.usage().cloned();
                let code = error.code().map(ToString::to_string);
                let details = error.details().cloned();
                let message = error.to_string();
                let runtime_terminal_trace = error.conversation_turn_trace().cloned();
                let previous_usage_state = self
                    .usage_contexts
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner())
                    .get(&run_id)
                    .cloned();
                let persisted = persist_terminal_with_bounded_retry(
                    || {
                        let deletion_lifecycle = self
                            .deletion_lifecycle
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner());
                        if deletion_lifecycle.contains_input(&record.agent_input) {
                            return Err(
                                "项目或会话正在移除，无法持久化审批续跑失败终态。".to_string()
                            );
                        }
                        if let (Some(conversation_id), Some(assistant_message_id)) = (
                            record.snapshot.conversation_id.as_deref(),
                            record.snapshot.assistant_message_id.as_deref(),
                        ) {
                            let terminal_projection = match runtime_terminal_trace.clone() {
                                Some(trace) => Ok((trace, None)),
                                None => self
                                    .storage
                                    .get_conversation_turn_trace(assistant_message_id)
                                    .and_then(|trace| match trace {
                                        Some(trace)
                                            if trace.run_id == run_id
                                                && trace.conversation_id == conversation_id
                                                && trace.terminal_status
                                                    == ConversationTurnTraceTerminalStatus::InProgress =>
                                        {
                                            let model_context_items = self
                                                .storage
                                                .get_conversation_model_context_log(
                                                    assistant_message_id,
                                                )?
                                                .map(|log| log.items)
                                                .unwrap_or_default();
                                            let next_sequence = trace
                                                .items
                                                .last()
                                                .map(ConversationTurnTraceItem::sequence)
                                                .unwrap_or(0)
                                                .saturating_add(1);
                                            let terminal =
                                                terminal_conversation_trace_from_snapshot(
                                                    ConversationTraceSnapshot {
                                                        items: trace.items,
                                                        model_context_items,
                                                        next_sequence,
                                                        truncated: trace.truncated,
                                                    },
                                                    &run_id,
                                                    conversation_id,
                                                    assistant_message_id,
                                                    ConversationTurnTraceTerminalStatus::Failed,
                                                    &message,
                                                )?;
                                            Ok((terminal.trace, Some(terminal.model_context_items)))
                                        }
                                        Some(_) => Err(
                                            "审批续跑的 durable ConversationTurnTrace 身份或状态不一致。"
                                                .to_string(),
                                        ),
                                        None => Ok((
                                            failed_conversation_trace_without_items(
                                                &run_id,
                                                conversation_id,
                                                assistant_message_id,
                                                &message,
                                            ),
                                            Some(Vec::new()),
                                        )),
                                    }),
                            };
                            terminal_projection.and_then(
                                |(conversation_turn_trace, model_context_items)| {
                                    if model_request_interruption.is_some() {
                                        self.persist_assistant_model_request_interruption_with_model_context(
                                            conversation_id,
                                            assistant_message_id,
                                            &message,
                                            usage.clone(),
                                            &conversation_turn_trace,
                                            model_context_items.as_deref(),
                                        )
                                    } else {
                                        self.persist_assistant_error_with_model_context(
                                            conversation_id,
                                            assistant_message_id,
                                            &message,
                                            usage.clone(),
                                            &conversation_turn_trace,
                                            model_context_items.as_deref(),
                                        )
                                    }
                                },
                            )
                        } else {
                            Err("审批续跑缺少 assistant 持久化身份。".to_string())
                        }
                    },
                    || restore_run_usage_state(self, &run_id, &previous_usage_state),
                )
                .await;
                let cumulative_usage = persisted.as_ref().ok().cloned().flatten();
                let pending_transition = if persisted.is_ok() {
                    persist_terminal_with_bounded_retry(
                        || {
                            let deletion_lifecycle = self
                                .deletion_lifecycle
                                .lock()
                                .unwrap_or_else(|lock_error| lock_error.into_inner());
                            if deletion_lifecycle.contains_input(&record.agent_input) {
                                return Err(
                                    "项目或会话正在移除，无法完成审批失败状态迁移。".to_string()
                                );
                            }
                            self.transition_pending_status(&record, final_pending_status)
                        },
                        || {},
                    )
                    .await
                } else {
                    Err(
                        "assistant 失败终态未持久化，已保留非终态 pending 记录供启动对账。"
                            .to_string(),
                    )
                };
                let settlement_committed = persisted.is_ok() && pending_transition.is_ok();
                if let Err(transition_error) = &pending_transition {
                    emit_pending_transition_error(
                        &notifications,
                        &run_id,
                        final_pending_status,
                        transition_error,
                    );
                }
                let terminal_commit_published = pending_terminal_commit_is_publishable(
                    persisted.is_ok(),
                    pending_transition.is_ok(),
                    true,
                );
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner());
                if deletion_lifecycle.contains_input(&record.agent_input) {
                    deletion_cleanup = true;
                    self.discard_usage_context(&run_id);
                } else if let Err(error) = &persisted {
                    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                        run_id: Some(run_id.clone()),
                        trace_sequence: None,
                        message: format!("无法原子持久化 assistant 失败终态与会话轨迹：{error}"),
                        recoverable: true,
                        code: Some("conversation_trace_persistence_failed".to_string()),
                        details: None,
                    }));
                } else if terminal_commit_published {
                    if let (Some(conversation_id), Some(assistant_message_id)) = (
                        record.snapshot.conversation_id.as_deref(),
                        record.snapshot.assistant_message_id.as_deref(),
                    ) {
                        self.notify_durable_turn_observers(assistant_message_id);
                        self.emit_terminal_context_window_snapshot(
                            &notifications,
                            &record.agent_input,
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            if model_request_interruption.is_some() {
                                ""
                            } else {
                                &message
                            },
                        );
                    }
                    let terminal_error_event = if let Some(reason) = model_request_interruption {
                        model_request_interruption_event(&run_id, reason)
                    } else {
                        AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            trace_sequence: None,
                            message: message.clone(),
                            recoverable: false,
                            code,
                            details,
                        }
                    };
                    let _ = notifications.send(agent_event_notification(terminal_error_event));
                    let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                        run_id: run_id.clone(),
                        success: false,
                        status: Some(AgentRunStatus::Failed),
                        content: model_request_interruption.is_none().then_some(message),
                        usage: cumulative_usage,
                        finish_reason: None,
                        proposed_actions: Vec::new(),
                    }));
                }
                (
                    terminal_commit_published && !deletion_cleanup,
                    settlement_committed,
                )
            }
        };

        if durable_turn_terminal || deletion_cleanup {
            self.release_conversation_turn_if_current(&turn_conversation_id, &run_id);
            self.release_turn_concurrency_permit(&run_id);
        }
        if deletion_cleanup || (durable_turn_terminal && !keep_trace_snapshot) {
            self.discard_trace_snapshot(&run_id);
            self.discard_exact_running_context_window_snapshot(&run_id);
        }
        if durable_turn_terminal
            || deletion_cleanup
            || (keep_trace_snapshot && settlement_committed)
        {
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
        }
    }
}

#[cfg(test)]
mod mcp_lifecycle_tests {
    use super::*;

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
}
