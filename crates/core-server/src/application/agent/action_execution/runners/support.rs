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

const BUILTIN_MCP_APPROVED_INVOCATION_WATCHDOG: Duration = Duration::from_secs(65);

async fn join_skill_script_worker<F>(worker: F) -> Result<(), tokio::task::JoinError>
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(worker).await
}

#[cfg(test)]
static SKILL_SCRIPT_WORKER_PANIC_TARGETS: Mutex<Vec<(std::sync::Weak<StorageService>, String)>> =
    Mutex::new(Vec::new());

#[cfg(test)]
static SKILL_SCRIPT_POST_RECEIPT_PANIC_TARGETS: Mutex<
    Vec<(std::sync::Weak<StorageService>, String)>,
> = Mutex::new(Vec::new());

#[cfg(test)]
fn inject_skill_script_worker_panic_once(storage: &Arc<StorageService>, action_id: &str) {
    let mut targets = SKILL_SCRIPT_WORKER_PANIC_TARGETS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    targets.retain(|(candidate_storage, _)| candidate_storage.strong_count() > 0);
    let storage = Arc::downgrade(storage);
    if !targets.iter().any(|(candidate_storage, candidate_id)| {
        candidate_storage.ptr_eq(&storage) && candidate_id == action_id
    }) {
        targets.push((storage, action_id.to_string()));
    }
}

#[cfg(test)]
fn skill_script_worker_panic_is_pending(storage: &Arc<StorageService>, action_id: &str) -> bool {
    let storage = Arc::downgrade(storage);
    SKILL_SCRIPT_WORKER_PANIC_TARGETS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .any(|(candidate_storage, candidate_id)| {
            candidate_storage.ptr_eq(&storage) && candidate_id == action_id
        })
}

#[cfg(test)]
fn maybe_panic_skill_script_worker_after_effect_boundary(
    storage: &Arc<StorageService>,
    action_id: &str,
) {
    let should_panic = {
        let storage = Arc::downgrade(storage);
        let mut targets = SKILL_SCRIPT_WORKER_PANIC_TARGETS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        targets
            .iter()
            .position(|(candidate_storage, candidate_id)| {
                candidate_storage.ptr_eq(&storage) && candidate_id == action_id
            })
            .map(|index| targets.swap_remove(index))
            .is_some()
    };
    if should_panic {
        panic!("injected Skill script worker panic after effect boundary");
    }
}

#[cfg(test)]
fn inject_skill_script_post_receipt_panic_once(storage: &Arc<StorageService>, action_id: &str) {
    let mut targets = SKILL_SCRIPT_POST_RECEIPT_PANIC_TARGETS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    targets.retain(|(candidate_storage, _)| candidate_storage.strong_count() > 0);
    let storage = Arc::downgrade(storage);
    if !targets.iter().any(|(candidate_storage, candidate_id)| {
        candidate_storage.ptr_eq(&storage) && candidate_id == action_id
    }) {
        targets.push((storage, action_id.to_string()));
    }
}

#[cfg(test)]
fn maybe_panic_skill_script_worker_after_receipt(storage: &Arc<StorageService>, action_id: &str) {
    let should_panic = {
        let storage = Arc::downgrade(storage);
        let mut targets = SKILL_SCRIPT_POST_RECEIPT_PANIC_TARGETS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        targets
            .iter()
            .position(|(candidate_storage, candidate_id)| {
                candidate_storage.ptr_eq(&storage) && candidate_id == action_id
            })
            .map(|index| targets.swap_remove(index))
            .is_some()
    };
    if should_panic {
        panic!("injected Skill script worker panic after durable receipt");
    }
}

fn skill_script_worker_failure_result(
    call_id: &str,
    effects_may_have_occurred: bool,
    worker_cancelled: bool,
) -> AgentToolResult {
    let (status, outcome, code, recovery, message) = if effects_may_have_occurred {
        (
            "outcome_unknown",
            "outcome_unknown",
            "skill_script.outcome_unknown",
            "inspectState",
            "The Skill script worker stopped after execution began. Workspace effects may have occurred; inspect durable state before retrying.",
        )
    } else {
        (
            "failed",
            "definitely_not_executed",
            if worker_cancelled {
                "skill_script.worker_cancelled_before_execution"
            } else {
                "skill_script.worker_failed_before_execution"
            },
            "retry",
            "The Skill script worker stopped before execution began.",
        )
    };
    AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.to_string(),
        tool: "skills_run_script".to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "skill_script",
            "status": status,
            "outcome": outcome,
            "code": code,
            "recovery": recovery,
            "effectsMayHaveOccurred": effects_may_have_occurred,
            "retryable": !effects_may_have_occurred,
        })),
        error: Some(message.to_string()),
    }
}

fn skill_script_setup_failure_result(
    call_id: &str,
    code: &str,
    recovery: &str,
    message: &str,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.to_string(),
        tool: "skills_run_script".to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "skill_script",
            "status": "failed",
            "outcome": "definitely_not_executed",
            "code": code,
            "recovery": recovery,
            "effectsMayHaveOccurred": false,
            "retryable": true,
        })),
        error: Some(message.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::application::agent) enum BuiltinMcpToolResultCommitDisposition {
    Committed,
    CommittedAndAdvanced,
    Terminalized,
}

async fn supervised_builtin_mcp_tool_result<F>(
    approval: &mycopilot_core::AgentBuiltinMcpToolApproval,
    timeout: Duration,
    invocation: F,
) -> AgentToolResult
where
    F: Future<Output = AgentResult<Value>> + Send + 'static,
{
    let mut task = tokio::spawn(invocation);
    let invocation_result = match tokio::time::timeout(timeout, &mut task).await {
        Ok(Ok(result)) => Some(result),
        Ok(Err(_)) => None,
        Err(_) => {
            task.abort();
            let _ = task.await;
            None
        }
    };
    match invocation_result {
        Some(Ok(value)) => AgentToolResult {
            exact_archive_file: None,
            call_id: approval.identity.call_id.clone(),
            tool: approval.identity.model_name.clone(),
            ok: true,
            result: Some(value),
            error: None,
        },
        Some(Err(error))
            if error
                .details()
                .and_then(|value| value.get("dispatchCertainty"))
                .and_then(Value::as_str)
                == Some("possibly_dispatched") =>
        {
            mycopilot_core::builtin_mcp_tool_outcome_unknown_result(approval)
        }
        Some(Err(error))
            if error.is_cancelled()
                && mcp_agent_error_dispatch_certainty(&error)
                    == AgentMcpDispatchCertainty::DefinitelyNotDispatched =>
        {
            mycopilot_core::builtin_mcp_tool_cancelled_result(approval)
        }
        Some(Err(error)) if error.is_cancelled() => {
            mycopilot_core::builtin_mcp_tool_outcome_unknown_result(approval)
        }
        Some(Err(error)) => AgentToolResult {
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
        None => mycopilot_core::builtin_mcp_tool_outcome_unknown_result(approval),
    }
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
                    let output_truncated = result.truncated_at_source
                        || mcp_tool_result_output_truncated(&tool_result);
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

fn verify_frozen_skill_script_source(
    script: &AgentSkillScriptRequest,
    resources: &SkillResourceSession,
) -> Result<VerifiedSkillResourceSource, String> {
    let uri = SkillResourceUri::parse(&script.script_uri)
        .map_err(|error| format!("The frozen Skill script URI is invalid: {error}"))?;
    if uri.package().skill_id().as_str() != script.skill_id
        || uri.package().revision().as_str() != script.skill_revision
        || uri.path().as_str() != script.resource_path
    {
        return Err(
            "The frozen Skill script identity does not match its canonical URI.".to_string(),
        );
    }
    let verified = resources
        .verify_resource_source(&uri)
        .map_err(|error| format!("The Skill script source could not be verified: {error}"))?;
    let kind_matches = matches!(
        (script.source.source_kind, verified.source_kind()),
        (
            AgentSkillScriptSourceKind::Workspace,
            SkillSourceKind::Workspace
        ) | (
            AgentSkillScriptSourceKind::Bundled,
            SkillSourceKind::Bundled
        ) | (
            AgentSkillScriptSourceKind::Installed,
            SkillSourceKind::Installed
        )
    );
    let trust_matches = matches!(
        (script.source.trust, verified.trust()),
        (AgentSkillScriptTrust::Untrusted, SkillTrust::Untrusted)
            | (
                AgentSkillScriptTrust::UserApproved,
                SkillTrust::UserApproved
            )
            | (AgentSkillScriptTrust::Application, SkillTrust::Application)
    );
    if verified.package() != uri.package()
        || verified.resource_digest() != script.resource_digest
        || verified.source_id().as_str() != script.source.source_id
        || !kind_matches
        || !trust_matches
    {
        return Err(
            "The frozen Skill script source proof no longer matches the active resource session."
                .to_string(),
        );
    }
    Ok(verified)
}

fn is_verified_application_bundled_source(source: &VerifiedSkillResourceSource) -> bool {
    source.source_id().as_str() == APPLICATION_BUNDLED_SKILL_SOURCE_ID
        && source.package().skill_id().source_id().as_str() == APPLICATION_BUNDLED_SKILL_SOURCE_ID
        && source.source_kind() == SkillSourceKind::Bundled
        && source.trust() == SkillTrust::Application
}
