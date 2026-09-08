#[derive(Debug, Clone, PartialEq, Eq)]
enum ManualSettlementTraceState {
    Absent,
    BeforeBoundary,
    AtBoundary,
    Advanced,
    Diverged(String),
}

fn manual_settlement_trace_state(
    durable: Option<&ConversationTurnTrace>,
    expected: &ConversationTurnTrace,
) -> ManualSettlementTraceState {
    let Some(durable) = durable else {
        return ManualSettlementTraceState::Absent;
    };
    if durable.schema_version != expected.schema_version
        || durable.run_id != expected.run_id
        || durable.conversation_id != expected.conversation_id
        || durable.assistant_message_id != expected.assistant_message_id
    {
        return ManualSettlementTraceState::Diverged(
            "the durable trace identity differs from the expected settlement".to_string(),
        );
    }
    if durable == expected {
        return ManualSettlementTraceState::AtBoundary;
    }
    if durable.items.len() < expected.items.len()
        && durable.items == expected.items[..durable.items.len()]
        && durable.terminal_status == crate::ConversationTurnTraceTerminalStatus::InProgress
        && durable.terminal_error == expected.terminal_error
        // Appending a bounded ToolResult may introduce the first truncated item. The
        // already committed prefix must remain exact, while this aggregate flag can only grow.
        && (!durable.truncated || expected.truncated)
    {
        return ManualSettlementTraceState::BeforeBoundary;
    }
    if expected.items.len() <= durable.items.len()
        && expected.items == durable.items[..expected.items.len()]
    {
        return ManualSettlementTraceState::Advanced;
    }
    ManualSettlementTraceState::Diverged(
        "the expected ToolResult boundary is not an exact prefix of the durable trace".to_string(),
    )
}

fn settlement_diverged(
    component: &'static str,
    reason: impl Into<String>,
) -> AgentPendingActionSettlementInspection {
    AgentPendingActionSettlementInspection::Diverged {
        component,
        reason: reason.into(),
    }
}

fn validate_durable_mcp_tool_result(
    tool_result: &AgentToolResult,
    target_status: &str,
) -> Result<bool, String> {
    let value = tool_result
        .result
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "MCP durable ToolResult must be a typed object".to_string())?;
    const ALLOWED_FIELDS: &[&str] = &[
        "schemaVersion",
        "type",
        "external",
        "status",
        "outcome",
        "dispatchCertainty",
        "contentOmitted",
        "isError",
        "feedbackProvided",
        "code",
        "retryable",
    ];
    if value
        .keys()
        .any(|key| !ALLOWED_FIELDS.contains(&key.as_str()))
    {
        return Err("MCP durable ToolResult contains an unknown field".to_string());
    }
    if value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || value.get("type").and_then(serde_json::Value::as_str) != Some("mcp_tool")
        || value.get("external").and_then(serde_json::Value::as_bool) != Some(true)
        || value
            .get("contentOmitted")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || tool_result.exact_archive_file.is_some()
    {
        return Err("MCP durable ToolResult is not the safe Host projection".to_string());
    }
    let code = value.get("code").and_then(serde_json::Value::as_str);
    if let Some(code) = code {
        if code.is_empty()
            || code.len() > 128
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err("MCP durable ToolResult has an invalid safe error code".to_string());
        }
    } else if value.contains_key("code") {
        return Err("MCP durable ToolResult error code must be a string".to_string());
    }
    let retryable = value.get("retryable").and_then(serde_json::Value::as_bool);
    if value.contains_key("retryable") && retryable.is_none() {
        return Err("MCP durable ToolResult retryable flag must be boolean".to_string());
    }

    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "MCP durable ToolResult lacks status".to_string())?;
    let outcome = value
        .get("outcome")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "MCP durable ToolResult lacks outcome".to_string())?;
    let dispatch_certainty = value
        .get("dispatchCertainty")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "MCP durable ToolResult lacks dispatch certainty".to_string())?;
    let is_error = value
        .get("isError")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| "MCP durable ToolResult lacks isError".to_string())?;
    let feedback_provided = value.get("feedbackProvided");

    let is_rejection = target_status == "rejected";
    let semantic_shape_is_valid = match target_status {
        "completed" => {
            status == "completed"
                && dispatch_certainty == "response_received"
                && matches!(
                    (outcome, is_error, tool_result.ok),
                    ("succeeded", false, true) | ("tool_error", true, false)
                )
                && feedback_provided.is_none()
                && code.is_none()
                && retryable.is_none()
        }
        "rejected" => {
            status == "rejected"
                && outcome == "rejected"
                && dispatch_certainty == "definitely_not_dispatched"
                && !is_error
                && tool_result.ok
                && feedback_provided.is_none_or(|feedback| feedback.as_bool() == Some(true))
                && code == Some("mcp.approval_rejected")
                && retryable == Some(false)
        }
        "cancelled" => {
            status == "cancelled"
                && outcome == "cancelled"
                && dispatch_certainty == "definitely_not_dispatched"
                && is_error
                && !tool_result.ok
                && feedback_provided.is_none()
                && code.is_some()
                && retryable.is_some()
        }
        "failed" => {
            !tool_result.ok
                && is_error
                && feedback_provided.is_none()
                && code.is_some()
                && retryable.is_some()
                && matches!(
                    (status, outcome),
                    ("failed", "output_too_large")
                        | ("failed", "transport_error")
                        | ("failed", "timed_out")
                        | ("expired", "expired")
                        | ("payload_unavailable", "payload_unavailable")
                        | ("policy_denied", "policy_denied")
                        | ("outcome_unknown", "outcome_unknown")
                )
                && matches!(
                    dispatch_certainty,
                    "definitely_not_dispatched" | "possibly_dispatched" | "response_received"
                )
        }
        _ => false,
    };
    if !semantic_shape_is_valid {
        return Err("MCP durable ToolResult terminal semantics are inconsistent".to_string());
    }

    const PERSISTED_MCP_ERROR: &str = "The external MCP tool reported an error.";
    if (tool_result.ok && tool_result.error.is_some())
        || (!tool_result.ok && tool_result.error.as_deref() != Some(PERSISTED_MCP_ERROR))
    {
        return Err("MCP durable ToolResult error projection is inconsistent".to_string());
    }
    Ok(is_rejection)
}

fn validate_durable_builtin_mcp_tool_result(
    tool_result: &AgentToolResult,
    target_status: &str,
) -> Result<bool, String> {
    let value = tool_result
        .result
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "built-in MCP durable ToolResult must be a typed object".to_string())?;
    const ALLOWED_FIELDS: &[&str] = &[
        "schemaVersion",
        "type",
        "status",
        "dispatchCertainty",
        "contentOmitted",
        "errorCode",
        "retryable",
        "artifacts",
    ];
    if value
        .keys()
        .any(|key| !ALLOWED_FIELDS.contains(&key.as_str()))
        || value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
        || value.get("type").and_then(serde_json::Value::as_str) != Some("builtin_capability_tool")
        || value
            .get("contentOmitted")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || tool_result.exact_archive_file.is_some()
    {
        return Err("built-in MCP durable ToolResult is not the safe Host projection".to_string());
    }
    let is_rejection = target_status == "rejected";
    if is_rejection
        && (value.get("status").and_then(serde_json::Value::as_str) != Some("rejected")
            || value
                .get("dispatchCertainty")
                .and_then(serde_json::Value::as_str)
                != Some("definitely_not_dispatched")
            || value.get("errorCode").and_then(serde_json::Value::as_str)
                != Some("mcp.approval_rejected")
            || value.get("retryable").and_then(serde_json::Value::as_bool) != Some(false)
            || !tool_result.ok
            || tool_result.error.is_some())
    {
        return Err("built-in MCP rejection ToolResult semantics are inconsistent".to_string());
    }
    Ok(is_rejection)
}

fn validate_manual_file_effect_settlement_request(
    audit: &AgentActionAuditRecord,
    expected_pending_status: &str,
    target_status: &str,
    trace: &ConversationTurnTrace,
    committed_at: i64,
) -> Result<(), String> {
    if !matches!(
        target_status,
        "completed" | "failed" | "cancelled" | "rejected"
    ) || audit.status != target_status
    {
        return Err(format!(
            "manual file-effect terminal status mismatch: audit={}, target={target_status}",
            audit.status
        ));
    }
    let action = serde_json::from_str::<AgentProposedAction>(&audit.action_json)
        .map_err(|error| format!("frozen file-effect action is invalid: {error}"))?;
    let is_external_mcp_action = matches!(action, AgentProposedAction::McpToolCall { .. });
    let is_builtin_mcp_action =
        matches!(action, AgentProposedAction::BuiltinMcpToolApproval { .. });
    let is_file_change = matches!(action, AgentProposedAction::FileChange { .. });
    let is_mcp_action = is_external_mcp_action || is_builtin_mcp_action;
    let is_mcp_rejection = is_mcp_action && target_status == "rejected";
    let is_file_change_cancellation = is_file_change && target_status == "cancelled";
    let valid_decision = if is_mcp_rejection {
        audit.decision.as_deref() == Some("rejected")
    } else if is_file_change_cancellation {
        audit.decision.as_deref() == Some("cancelled")
    } else {
        audit.decision.as_deref() == Some("approved")
    };
    let valid_decision_source = audit.decision_source.as_deref() == Some("manual")
        || (is_file_change
            && matches!(audit.decision_source.as_deref(), Some("auto" | "run_grant")));
    if !valid_decision || !valid_decision_source {
        return Err("file-effect settlement contains an invalid audit lifecycle".to_string());
    }
    let (expected_action_type, expected_tool, expected_call_id, is_command) =
        manual_file_effect_identity(&action)?;
    let pending_status_is_valid = (is_mcp_rejection && expected_pending_status == "pending")
        || expected_pending_status == "approved"
        || (expected_action_type == "skill_materialization"
            && expected_pending_status == "executing")
        || (expected_action_type == "file_change" && expected_pending_status == "executing")
        || (expected_action_type == "skill_script" && expected_pending_status == "executing")
        || (expected_action_type == "mcp_tool_call" && expected_pending_status == "executing")
        || (expected_action_type == "builtin_mcp_tool_approval"
            && expected_pending_status == "executing");
    if !pending_status_is_valid {
        return Err(format!(
            "manual file-effect settlement has invalid pending status `{expected_pending_status}` for `{expected_action_type}`"
        ));
    }
    if audit.action_type != expected_action_type || audit.tool_name != expected_tool {
        return Err("manual file-effect audit type does not match the frozen action".to_string());
    }
    let completed_at = audit
        .completed_at
        .ok_or_else(|| "manual file-effect terminal audit lacks completed_at".to_string())?;
    if completed_at < audit.created_at || committed_at != completed_at {
        return Err("manual file-effect settlement timestamps are inconsistent".to_string());
    }
    let tool_result_json = audit
        .tool_result_json
        .as_deref()
        .ok_or_else(|| "manual file-effect terminal audit lacks tool_result_json".to_string())?;
    let tool_result = serde_json::from_str::<AgentToolResult>(tool_result_json)
        .map_err(|error| format!("manual file-effect ToolResult is invalid: {error}"))?;
    match (is_file_change, audit.file_change_result_json.as_deref()) {
        (true, Some(result_json)) => {
            let result = serde_json::from_str::<crate::AgentFileChangeResult>(result_json)
                .map_err(|_| "FileChange terminal result is invalid".to_string())?;
            let AgentProposedAction::FileChange { file_change } = &action else {
                unreachable!("is_file_change was derived from the exact action")
            };
            if !crate::file_change_support::file_change_result_matches_frozen_proposal(
                &result,
                file_change,
            ) {
                return Err(
                    "FileChange terminal result differs from its frozen proposal".to_string(),
                );
            }
            if tool_result.result.as_ref()
                != Some(
                    &serde_json::to_value(&result).map_err(|_| {
                        "FileChange terminal result could not be validated".to_string()
                    })?,
                )
            {
                return Err("FileChange terminal audit differs from its ToolResult".to_string());
            }
        }
        (true, None) => {
            return Err("FileChange terminal audit lacks file_change_result_json".to_string())
        }
        (false, Some(_)) => {
            return Err("non-FileChange audit contains file_change_result_json".to_string())
        }
        (false, None) => {}
    }
    let command_handoff = is_command
        && audit.command_result_json.is_none()
        && target_status == "completed"
        && validate_manual_command_handoff_projection(&tool_result).is_ok();
    let command_result = match (is_command, audit.command_result_json.as_deref()) {
        (true, Some(command_result_json)) => Some(
            serde_json::from_str::<AgentCommandExecutionResult>(command_result_json)
                .map_err(|error| format!("manual command result is invalid: {error}"))?,
        ),
        (true, None) if command_handoff => None,
        (true, None) => {
            return Err("manual command terminal audit lacks command_result_json".to_string());
        }
        (false, None) => None,
        (false, Some(_)) => {
            return Err(
                "non-command manual file-effect audit unexpectedly contains command_result_json"
                    .to_string(),
            );
        }
    };
    if let Some(command_result) = command_result.as_ref() {
        validate_manual_command_result_projection(command_result, &tool_result, target_status)?;
    } else if command_handoff {
        validate_manual_command_handoff_projection(&tool_result)?;
    }
    if is_mcp_action {
        let validated_rejection = if is_external_mcp_action {
            validate_durable_mcp_tool_result(&tool_result, target_status)?
        } else {
            validate_durable_builtin_mcp_tool_result(&tool_result, target_status)?
        };
        if validated_rejection != is_mcp_rejection {
            return Err("MCP durable ToolResult rejection state is inconsistent".to_string());
        }
    }
    let expected_ok = if is_mcp_action || is_file_change_cancellation {
        tool_result.ok
    } else {
        target_status == "completed"
    };
    if tool_result.tool != expected_tool
        || tool_result.call_id != expected_call_id
        || expected_ok != tool_result.ok
        || audit.error != tool_result.error
    {
        return Err("manual file-effect audit and ToolResult terminal state differ".to_string());
    }
    if trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress {
        return Err("manual file-effect settlement trace must remain in_progress".to_string());
    }
    trace.validate()?;
    let Some(ConversationTurnTraceItem::ToolResult {
        call_id,
        tool,
        success,
        ..
    }) = trace.items.last()
    else {
        return Err("manual file-effect settlement trace lacks a final ToolResult".to_string());
    };
    // A rejected MCP approval is a successfully delivered Host ToolResult for the model, while
    // the append-only trace deliberately records the external operation itself as not succeeded.
    // Keeping those two meanings separate prevents rejection from becoming an execution error
    // without falsely claiming that the MCP tool ran.
    let expected_trace_success = tool_result.ok && !is_mcp_rejection;
    if call_id != &tool_result.call_id
        || tool != &expected_tool
        || *success != expected_trace_success
    {
        return Err(
            "manual file-effect trace result identity differs from terminal audit".to_string(),
        );
    }
    Ok(())
}

fn validate_manual_command_handoff_projection(tool_result: &AgentToolResult) -> Result<(), String> {
    if tool_result.tool != "run_command"
        || !tool_result.ok
        || tool_result.error.is_some()
        || tool_result.exact_archive_file.is_some()
    {
        return Err("manual command handoff ToolResult has an invalid envelope".to_string());
    }
    let result = tool_result
        .result
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "manual command handoff ToolResult lacks an object result".to_string())?;
    const KEYS: [&str; 7] = [
        "status",
        "sessionId",
        "output",
        "startedAt",
        "latestSequence",
        "outputTruncated",
        "continueWith",
    ];
    let session_id = result
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let session_hex = session_id.strip_prefix("cmd_");
    let continue_with = result
        .get("continueWith")
        .and_then(serde_json::Value::as_object);
    let continue_with_args = continue_with
        .and_then(|continuation| continuation.get("args"))
        .and_then(serde_json::Value::as_object);
    const CONTINUE_WITH_KEYS: [&str; 2] = ["tool", "args"];
    const CONTINUE_WITH_ARG_KEYS: [&str; 2] = ["sessionId", "action"];
    if result.len() != KEYS.len()
        || KEYS.iter().any(|key| !result.contains_key(*key))
        || result.get("status").and_then(serde_json::Value::as_str) != Some("running")
        || session_hex.is_none_or(|value| {
            value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || result
            .get("output")
            .and_then(serde_json::Value::as_str)
            .is_none()
        || result
            .get("startedAt")
            .and_then(serde_json::Value::as_u64)
            .is_none()
        || result
            .get("latestSequence")
            .and_then(serde_json::Value::as_u64)
            .is_none()
        || result
            .get("outputTruncated")
            .and_then(serde_json::Value::as_bool)
            .is_none()
        || continue_with.is_none_or(|continuation| {
            continuation.len() != CONTINUE_WITH_KEYS.len()
                || CONTINUE_WITH_KEYS
                    .iter()
                    .any(|key| !continuation.contains_key(*key))
                || continuation.get("tool").and_then(serde_json::Value::as_str)
                    != Some("command_session")
        })
        || continue_with_args.is_none_or(|args| {
            args.len() != CONTINUE_WITH_ARG_KEYS.len()
                || CONTINUE_WITH_ARG_KEYS
                    .iter()
                    .any(|key| !args.contains_key(*key))
                || args.get("sessionId").and_then(serde_json::Value::as_str) != Some(session_id)
                || args.get("action").and_then(serde_json::Value::as_str) != Some("wait")
        })
    {
        return Err("manual command handoff ToolResult is invalid".to_string());
    }
    Ok(())
}

fn validate_manual_command_result_projection(
    command_result: &AgentCommandExecutionResult,
    tool_result: &AgentToolResult,
    target_status: &str,
) -> Result<(), String> {
    let canonical = crate::command::command_tool_result(&tool_result.call_id, command_result);
    let execution = canonical
        .result
        .as_ref()
        .expect("canonical command ToolResult always contains execution evidence");
    if tool_result.result.as_ref() == Some(execution) {
        if tool_result.ok != canonical.ok || tool_result.error != canonical.error {
            return Err(
                "manual command ToolResult terminal outcome differs from its execution evidence"
                    .to_string(),
            );
        }
        let expected_target_status = if command_result.cancelled {
            "cancelled"
        } else if canonical.ok {
            "completed"
        } else {
            "failed"
        };
        if target_status != expected_target_status {
            return Err(format!(
                "manual command terminal target `{target_status}` differs from execution outcome `{expected_target_status}`"
            ));
        }
        return Ok(());
    }

    let Some(wrapper) = tool_result
        .result
        .as_ref()
        .and_then(serde_json::Value::as_object)
    else {
        return Err("manual command ToolResult omits its execution evidence".to_string());
    };
    const WRAPPER_KEYS: [&str; 8] = [
        "type",
        "code",
        "recovery",
        "phase",
        "executionAttempted",
        "effectsMayHaveOccurred",
        "auditError",
        "execution",
    ];
    if wrapper.len() != WRAPPER_KEYS.len()
        || WRAPPER_KEYS.iter().any(|key| !wrapper.contains_key(*key))
        || wrapper.get("type").and_then(serde_json::Value::as_str)
            != Some("command_execution")
        || wrapper.get("code").and_then(serde_json::Value::as_str)
            != Some("auditPersistenceFailed")
        || wrapper.get("recovery").and_then(serde_json::Value::as_str)
            != Some("inspectArtifacts")
        || wrapper.get("phase").and_then(serde_json::Value::as_str) != Some("afterExecution")
        || wrapper
            .get("executionAttempted")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || wrapper
            .get("effectsMayHaveOccurred")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || wrapper
            .get("auditError")
            .and_then(serde_json::Value::as_str)
            .is_none()
        || !matches!(
            wrapper.get("execution"),
            Some(value) if value == execution
        )
        || target_status != "failed"
        || tool_result.ok
        || tool_result.error.as_deref()
            != Some(
                "The command finished, but its final action audit could not be persisted. Inspect the observed artifacts before retrying.",
            )
    {
        return Err(
            "manual command ToolResult differs from its durable command execution result"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_manual_file_effect_settlement_identity(
    pending: &AgentPendingActionRecord,
    audit: &AgentActionAuditRecord,
    expected_pending_status: &str,
    target_status: &str,
    trace: &ConversationTurnTrace,
) -> Result<(), String> {
    let tool_result = serde_json::from_str::<AgentToolResult>(
        audit
            .tool_result_json
            .as_deref()
            .expect("request validation requires tool_result_json"),
    )
    .map_err(|error| format!("manual file-effect ToolResult is invalid: {error}"))?;
    if pending.status != expected_pending_status
        || pending.action_id != audit.action_id
        || pending.run_id != audit.run_id
        || pending.conversation_id != audit.conversation_id
        || pending.assistant_message_id != audit.assistant_message_id
        || pending.action_type != audit.action_type
        || pending.tool_name != audit.tool_name
        || pending.tool_call_id.as_deref() != Some(tool_result.call_id.as_str())
        || pending.action_json != audit.action_json
        || pending.created_at != audit.created_at
    {
        return Err(
            "manual file-effect settlement does not match the frozen pending action".to_string(),
        );
    }
    if pending
        .target_status
        .as_deref()
        .is_some_and(|existing| existing != target_status)
    {
        return Err(format!(
            "manual file-effect pending action already targets a different terminal status: {}",
            pending.target_status.as_deref().unwrap_or_default()
        ));
    }
    if trace.run_id != pending.run_id
        || Some(trace.conversation_id.as_str()) != pending.conversation_id.as_deref()
        || Some(trace.assistant_message_id.as_str()) != pending.assistant_message_id.as_deref()
    {
        return Err(
            "manual file-effect settlement trace identity differs from pending action".to_string(),
        );
    }
    let action = serde_json::from_str::<AgentProposedAction>(&pending.action_json)
        .map_err(|error| format!("frozen file-effect action is invalid: {error}"))?;
    let (_, expected_tool, expected_call_id, _) = manual_file_effect_identity(&action)?;
    if expected_call_id != tool_result.call_id || expected_tool != tool_result.tool {
        return Err("frozen file-effect identity differs from terminal ToolResult".to_string());
    }
    Ok(())
}

fn manual_file_effect_identity(
    action: &AgentProposedAction,
) -> Result<(&'static str, String, String, bool), String> {
    match action {
        AgentProposedAction::FileChange { file_change } => {
            if file_change.execution.validate().is_err()
                || file_change.schema_version != crate::file_change::FILE_CHANGE_SCHEMA_VERSION
                || file_change.execution.source_tool_name != "apply_patch"
                || file_change.id != file_change.execution.source_call_id
                || file_change.transaction_id != file_change.execution.transaction.id
            {
                return Err("manual FileChange identity is invalid".to_string());
            }
            Ok((
                "file_change",
                "apply_patch".to_string(),
                file_change.id.clone(),
                false,
            ))
        }
        AgentProposedAction::Command { command } => Ok((
            "command",
            "run_command".to_string(),
            command.id.clone(),
            true,
        )),
        AgentProposedAction::SkillMaterialization { materialization } => Ok((
            "skill_materialization",
            "skills_materialize_resource".to_string(),
            materialization.id.clone(),
            false,
        )),
        AgentProposedAction::SkillScript { script } => Ok((
            "skill_script",
            "skills_run_script".to_string(),
            script.id.clone(),
            false,
        )),
        AgentProposedAction::OfficeOperation { office_operation } => {
            let tool = match office_operation.prepared.request.document_kind {
                crate::office::OfficeDocumentKind::Document => "office_document",
                crate::office::OfficeDocumentKind::Spreadsheet => "office_spreadsheet",
                crate::office::OfficeDocumentKind::Presentation => "office_presentation",
            };
            Ok((
                "office_operation",
                tool.to_string(),
                office_operation.id.clone(),
                false,
            ))
        }
        AgentProposedAction::McpToolCall { approval } => Ok((
            "mcp_tool_call",
            approval.identity.provenance.model_tool_name.clone(),
            approval.identity.call_id.clone(),
            false,
        )),
        AgentProposedAction::BuiltinMcpToolApproval { approval } => Ok((
            "builtin_mcp_tool_approval",
            approval.identity.model_name.clone(),
            approval.identity.call_id.clone(),
            false,
        )),
        _ => Err("manual audited settlement does not support this action type".to_string()),
    }
}
