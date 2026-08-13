use super::*;

#[derive(Debug)]
struct DurablePendingTraceSnapshot {
    snapshot: crate::ConversationTraceSnapshot,
    call: AgentToolCall,
    provenance: crate::AgentToolIdentity,
}

/// Loads the canonical durable recovery identity for one pending row.
///
/// The pending resume envelope is deliberately not consulted. Startup terminalization needs only
/// the immutable trace, its exact staged Provider/Runtime ToolCall model item, and the row owner
/// identity. A malformed private resume payload can therefore be scrubbed without becoming an
/// alternate source of Tool identity.
fn load_durable_pending_trace_snapshot(
    connection: &rusqlite::Connection,
    record: &AgentPendingActionRecord,
    require_open_call: bool,
) -> Result<DurablePendingTraceSnapshot, String> {
    let conversation_id = record
        .conversation_id
        .as_deref()
        .ok_or_else(|| "pending action requires a conversation owner".to_string())?;
    let assistant_message_id = record
        .assistant_message_id
        .as_deref()
        .ok_or_else(|| "pending action requires an Assistant owner".to_string())?;
    let trace =
        conversation_trace_repository::get_trace_for_message(connection, assistant_message_id)
            .map_err(storage_error)?
            .ok_or_else(|| "pending action requires a durable ConversationTurnTrace".to_string())?;
    trace
        .validate()
        .map_err(|_| "pending action durable ConversationTurnTrace is invalid".to_string())?;
    if trace.run_id != record.run_id
        || trace.conversation_id != conversation_id
        || trace.assistant_message_id != assistant_message_id
        || trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress
    {
        return Err("pending action durable ConversationTurnTrace identity is inconsistent".into());
    }

    let row_call_id = record
        .tool_call_id
        .as_deref()
        .ok_or_else(|| "pending action requires a ToolCall identity".to_string())?;
    let matching_calls = trace
        .items
        .iter()
        .filter_map(|item| {
            let ConversationTurnTraceItem::ToolCall {
                sequence,
                call_id,
                tool,
                provenance,
                operation,
                approval_status,
                ..
            } = item
            else {
                return None;
            };
            (call_id == row_call_id).then(|| {
                let closed = trace.items.iter().any(|candidate| {
                matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
                });
                (
                    *sequence,
                    AgentToolCall {
                        id: call_id.clone(),
                        tool: tool.clone(),
                        args: operation.clone(),
                        approval_status: *approval_status,
                        reason: operation
                            .get("reason")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                    },
                    provenance.clone(),
                    !closed,
                )
            })
        })
        .collect::<Vec<_>>();
    let [(call_sequence, call, provenance, call_is_open)] = matching_calls.as_slice() else {
        return Err("pending action requires exactly one matching durable ToolCall".to_string());
    };
    let unrelated_open_call = trace.items.iter().any(|item| {
        let ConversationTurnTraceItem::ToolCall { call_id, .. } = item else {
            return false;
        };
        call_id != row_call_id
            && !trace.items.iter().any(|candidate| {
                matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
            })
    });
    if record.tool_name != call.tool || unrelated_open_call {
        return Err("pending action row does not match its durable open ToolCall".to_string());
    }
    if require_open_call && !call_is_open {
        return Err("pending action requires exactly one durable open ToolCall".to_string());
    }

    let model_context_items = conversation_model_context_repository::get_log_for_message(
        connection,
        assistant_message_id,
    )
    .map_err(storage_error)?
    .map(|log| log.items)
    .ok_or_else(|| "pending action requires a durable model-context log".to_string())?;
    crate::conversation_trace::validate_model_context_prefix(&trace, &model_context_items)
        .map_err(|_| "pending action durable model-context log is invalid".to_string())?;
    let exact_open_model_items = model_context_items
        .iter()
        .filter(|item| {
            item.sequence == *call_sequence
                && item.role == "assistant"
                && item.tool_call_id.is_none()
                && item.tool_calls.len() == 1
                && item.tool_calls[0].id == call.id
                && item.tool_calls[0].name == call.tool
        })
        .count();
    if exact_open_model_items != 1 {
        return Err(
            "pending action requires the exact durable Provider/Runtime ToolCall identity"
                .to_string(),
        );
    }
    if !call_is_open {
        trace
            .validate_complete_model_context(&model_context_items)
            .map_err(|_| "pending action closed model-context log is incomplete".to_string())?;
    }
    let next_sequence = trace
        .items
        .last()
        .map(ConversationTurnTraceItem::sequence)
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| "pending action durable trace sequence is exhausted".to_string())?;
    Ok(DurablePendingTraceSnapshot {
        snapshot: crate::ConversationTraceSnapshot {
            items: trace.items,
            model_context_items,
            next_sequence,
            truncated: trace.truncated,
        },
        call: call.clone(),
        provenance: provenance.clone(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPendingActionResultCommitOutcome {
    Committed { trace_changed: bool },
    Idempotent,
}

/// Authoritative durable state of one attempted manual-command settlement.
///
/// The inspection reads the pending target, action audit, and conversation trace from one SQLite
/// snapshot. Callers may manufacture a fallback failure only for `DefinitelyUncommitted`;
/// partial or conflicting state is deliberately indeterminate and must not be overwritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentPendingActionSettlementInspection {
    CommittedAtBoundary,
    CommittedAndAdvanced,
    DefinitelyUncommitted,
    Diverged {
        component: &'static str,
        reason: String,
    },
}

/// Safe terminal classification for an MCP approval found during Host startup.
///
/// The storage boundary accepts this closed enum rather than arbitrary messages, ensuring neither
/// model-authored arguments nor Server diagnostics can enter the durable audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpStartupActionTerminalOutcome {
    PayloadUnavailable,
    Expired,
    PolicyDenied,
    Rejected,
    Cancelled,
    OutcomeUnknown,
}

/// Terminal state for the internal journal used by automatically authorized MCP calls.
///
/// This journal records only the frozen, Renderer-safe call identity. It deliberately excludes
/// Tool arguments and results; terminal settlement scrubs the remaining action/input projection
/// and any one-time payload envelope in the same transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAutoActionJournalTerminalOutcome {
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

impl McpAutoActionJournalTerminalOutcome {
    fn pending_status(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed | Self::OutcomeUnknown => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn error_code(self) -> Option<&'static str> {
        match self {
            Self::OutcomeUnknown => Some("mcp.tool_outcome_unknown"),
            Self::Completed | Self::Failed | Self::Cancelled => None,
        }
    }

    fn safe_reason(self) -> Option<&'static str> {
        match self {
            Self::OutcomeUnknown => Some(
                "The automatic MCP invocation crossed the durable dispatch boundary, but no authoritative Tool response was received; it was not replayed.",
            ),
            Self::Completed | Self::Failed | Self::Cancelled => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpActionTerminalizationRequest {
    pub action_id: String,
    pub expected_status: String,
    pub outcome: McpStartupActionTerminalOutcome,
}

impl McpStartupActionTerminalOutcome {
    fn error_code(self) -> &'static str {
        match self {
            Self::PayloadUnavailable => "mcp.approval_payload_unavailable",
            Self::Expired => "mcp.approval_payload_expired",
            Self::PolicyDenied => "mcp.approval_policy_denied",
            Self::Rejected => "mcp.approval_rejected",
            Self::Cancelled => "mcp.approval_cancelled",
            Self::OutcomeUnknown => "mcp.tool_outcome_unknown",
        }
    }

    fn safe_reason(self) -> &'static str {
        match self {
            Self::PayloadUnavailable => {
                "The sealed MCP approval payload was unavailable after process restart; the tool was definitely not dispatched."
            }
            Self::Expired => {
                "The MCP approval expired before dispatch; the tool was definitely not dispatched."
            }
            Self::PolicyDenied => {
                "Host policy invalidated the MCP approval before dispatch; the tool was definitely not dispatched."
            }
            Self::Rejected => {
                "The recovered MCP approval was rejected by the user before dispatch; the tool was definitely not dispatched."
            }
            Self::Cancelled => {
                "The recovered MCP approval was cancelled by the user before dispatch; the tool was definitely not dispatched."
            }
            Self::OutcomeUnknown => {
                "The MCP invocation crossed the durable dispatch boundary before process restart; its outcome is unknown and it was not replayed."
            }
        }
    }

    fn pending_status(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::PayloadUnavailable
            | Self::Expired
            | Self::PolicyDenied
            | Self::OutcomeUnknown => "failed",
        }
    }

    fn run_status(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::PayloadUnavailable
            | Self::Expired
            | Self::PolicyDenied
            | Self::Rejected
            | Self::OutcomeUnknown => "failed",
        }
    }

    fn message_status(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::PayloadUnavailable
            | Self::Expired
            | Self::PolicyDenied
            | Self::Rejected
            | Self::OutcomeUnknown => "error",
        }
    }

    fn invocation_state(self) -> &'static str {
        match self {
            Self::PayloadUnavailable => "payload_unavailable",
            Self::Expired => "expired",
            Self::PolicyDenied => "policy_denied",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    fn invocation_outcome(self) -> &'static str {
        match self {
            Self::PayloadUnavailable => "payload_unavailable",
            Self::Expired => "expired",
            Self::PolicyDenied => "policy_denied",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    fn dispatch_certainty(self) -> &'static str {
        match self {
            Self::OutcomeUnknown => "possibly_dispatched",
            Self::PayloadUnavailable
            | Self::Expired
            | Self::PolicyDenied
            | Self::Rejected
            | Self::Cancelled => "definitely_not_dispatched",
        }
    }

    fn invocation_is_error(self) -> Option<bool> {
        match self {
            Self::PayloadUnavailable => Some(true),
            Self::Expired
            | Self::PolicyDenied
            | Self::Rejected
            | Self::Cancelled
            | Self::OutcomeUnknown => None,
        }
    }
}

fn valid_mcp_terminal_transition(
    expected_status: &str,
    outcome: McpStartupActionTerminalOutcome,
) -> bool {
    match outcome {
        McpStartupActionTerminalOutcome::PayloadUnavailable
        | McpStartupActionTerminalOutcome::Expired
        | McpStartupActionTerminalOutcome::PolicyDenied
        | McpStartupActionTerminalOutcome::Rejected
        | McpStartupActionTerminalOutcome::Cancelled => {
            matches!(expected_status, "pending" | "approved")
        }
        McpStartupActionTerminalOutcome::OutcomeUnknown => expected_status == "executing",
    }
}

fn is_valid_pending_successor(
    connection: &rusqlite::Connection,
    interrupted: &AgentPendingActionRecord,
    candidate: &AgentPendingActionRecord,
) -> Result<bool, String> {
    if !is_pending_successor_candidate(interrupted, candidate) {
        return Ok(false);
    }
    let Ok(action) = serde_json::from_str::<AgentProposedAction>(&candidate.action_json) else {
        return Ok(false);
    };
    let action_id = match &action {
        AgentProposedAction::ToolCall { call } => call.id.as_str(),
        AgentProposedAction::McpToolCall { approval } => approval.identity.call_id.as_str(),
        AgentProposedAction::Diff { diff } => diff.id.as_str(),
        AgentProposedAction::FileWrite { file_write } => file_write.id.as_str(),
        AgentProposedAction::Command { command } => command.id.as_str(),
        AgentProposedAction::SkillMaterialization { materialization } => {
            materialization.id.as_str()
        }
        AgentProposedAction::SkillScript { script } => script.id.as_str(),
        AgentProposedAction::OfficeOperation { office_operation } => office_operation.id.as_str(),
        AgentProposedAction::SkillInstallation { installation } => installation.id.as_str(),
    };
    if candidate.tool_call_id.as_deref() != Some(action_id) {
        return Ok(false);
    }
    let Some(assistant_message_id) = candidate.assistant_message_id.as_deref() else {
        return Ok(false);
    };
    let Some(trace) =
        conversation_trace_repository::get_trace_for_message(connection, assistant_message_id)
            .map_err(storage_error)?
    else {
        return Ok(false);
    };
    if trace.run_id != candidate.run_id
        || trace.conversation_id != candidate.conversation_id.as_deref().unwrap_or_default()
        || trace.assistant_message_id != assistant_message_id
    {
        return Ok(false);
    }
    let Some(parent_call_id) = interrupted.tool_call_id.as_deref() else {
        return Ok(false);
    };
    let parent_result_sequence = trace.items.iter().find_map(|item| match item {
        ConversationTurnTraceItem::ToolResult {
            sequence, call_id, ..
        } if call_id == parent_call_id => Some(sequence.to_owned()),
        _ => None,
    });
    let child_call_sequence = trace.items.iter().find_map(|item| match item {
        ConversationTurnTraceItem::ToolCall {
            sequence, call_id, ..
        } if call_id == action_id => Some(sequence.to_owned()),
        _ => None,
    });
    Ok(matches!(
        (parent_result_sequence, child_call_sequence),
        (Some(parent), Some(child)) if parent < child
    ))
}

pub(super) fn is_pending_successor_candidate(
    interrupted: &AgentPendingActionRecord,
    candidate: &AgentPendingActionRecord,
) -> bool {
    candidate.status == "pending"
        && candidate.action_id != interrupted.action_id
        && candidate.run_id == interrupted.run_id
        && candidate.conversation_id == interrupted.conversation_id
        && candidate.assistant_message_id == interrupted.assistant_message_id
}

fn validate_frozen_manual_file_effect_tool_call(
    action_id: &str,
    action: &AgentProposedAction,
    tool: &str,
    operation: &serde_json::Value,
) -> Result<Option<String>, String> {
    let (_, expected_tool, _, _) = manual_file_effect_identity(action)?;
    if tool != expected_tool {
        return Err(format!(
            "启动对账发现人工命令 {action_id} 的冻结 ToolCall 名称不一致。"
        ));
    }

    let reason = match action {
        AgentProposedAction::Command { command } => {
            crate::tools::validate_frozen_command_trace_args(command, operation).map_err(
                |error| {
                    format!(
                        "启动对账发现人工命令 {action_id} 的冻结 ToolCall 参数与 action 不一致：{error}"
                    )
                },
            )?;
            command.reason.clone()
        }
        AgentProposedAction::SkillMaterialization { materialization } => {
            crate::tools::validate_frozen_materialization_trace_args(materialization, operation)
                .map_err(|error| {
                    format!(
                        "启动对账发现人工文件副作用 {action_id} 的冻结 ToolCall 参数与 action 不一致：{error}"
                    )
                })?;
            materialization.reason.clone()
        }
        AgentProposedAction::SkillScript { script } => {
            crate::tools::validate_frozen_skill_script_trace_args(script, operation).map_err(
                |error| {
                    format!(
                        "启动对账发现人工文件副作用 {action_id} 的冻结 ToolCall 参数与 action 不一致：{error}"
                    )
                },
            )?;
            script.reason.clone()
        }
        AgentProposedAction::OfficeOperation { office_operation } => {
            validate_frozen_office_settlement_trace_args(office_operation, operation).map_err(
                |error| {
                    format!(
                        "启动对账发现人工文件副作用 {action_id} 的冻结 ToolCall 参数与 action 不一致：{error}"
                    )
                },
            )?;
            Some(office_operation.reason.clone())
        }
        _ => unreachable!("manual_file_effect_identity already rejected this action"),
    };
    Ok(reason)
}

fn validate_frozen_office_settlement_trace_args(
    frozen: &crate::AgentOfficeOperationRequest,
    operation: &serde_json::Value,
) -> Result<(), String> {
    crate::tools::validate_frozen_office_trace_args(frozen, operation)
}

/// Proves that a manual file-producing action has one coherent terminal receipt.
///
/// The repository query deliberately returns every write-ahead terminal target as a candidate.
/// This verifier is the trust boundary which excludes genuinely settled rows: the frozen action,
/// terminal audit, and paired trace item must all identify the same tool call and outcome. Any
/// malformed or incomplete evidence is conservatively reported as unsettled rather than inferred
/// from `pending.status` alone.
fn manual_file_effect_has_authoritative_settlement(
    connection: &rusqlite::Connection,
    pending: &AgentPendingActionRecord,
) -> Result<bool, String> {
    let Some(target_status @ ("completed" | "failed" | "cancelled")) =
        pending.target_status.as_deref()
    else {
        return Ok(false);
    };
    let Some(audit) =
        agent_action_audit_repository::load_action_audit_record(connection, &pending.action_id)
            .map_err(storage_error)?
    else {
        return Ok(false);
    };
    let Some(assistant_message_id) = pending.assistant_message_id.as_deref() else {
        return Ok(false);
    };
    let Some(durable_trace) =
        conversation_trace_repository::get_trace_for_message(connection, assistant_message_id)
            .map_err(storage_error)?
    else {
        return Ok(false);
    };

    let proof = (|| {
        durable_trace.validate()?;
        let action = serde_json::from_str::<AgentProposedAction>(&pending.action_json)
            .map_err(|error| format!("frozen file-effect action is invalid: {error}"))?;
        let (_, expected_tool, expected_call_id, _) = manual_file_effect_identity(&action)?;

        let matching_calls = durable_trace
            .items
            .iter()
            .filter_map(|item| match item {
                ConversationTurnTraceItem::ToolCall {
                    sequence,
                    call_id,
                    tool,
                    operation,
                    approval_status,
                    ..
                } if call_id == &expected_call_id => {
                    Some((*sequence, tool, operation, *approval_status))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if matching_calls.len() != 1 {
            return Err("durable trace must contain one matching ToolCall".to_string());
        }
        let (call_sequence, trace_tool, operation, call_approval_status) = matching_calls[0];
        if call_approval_status != crate::AgentApprovalStatus::Approved {
            return Err("durable trace ToolCall is not approved".to_string());
        }
        let reason = validate_frozen_manual_file_effect_tool_call(
            &pending.action_id,
            &action,
            trace_tool,
            operation,
        )?;

        let matching_results = durable_trace
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item {
                ConversationTurnTraceItem::ToolResult {
                    sequence, call_id, ..
                } if call_id == &expected_call_id => Some((index, *sequence)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if matching_results.len() != 1 {
            return Err("durable trace must contain one matching ToolResult".to_string());
        }
        let (result_index, result_sequence) = matching_results[0];
        if result_sequence <= call_sequence {
            return Err("durable trace result does not match its frozen ToolCall".to_string());
        }
        let audited_tool_result =
            serde_json::from_str::<AgentToolResult>(audit.tool_result_json.as_deref().ok_or_else(
                || "manual file-effect terminal audit lacks tool_result_json".to_string(),
            )?)
            .map_err(|error| format!("manual file-effect ToolResult is invalid: {error}"))?;
        let expected_call = AgentToolCall {
            id: expected_call_id,
            tool: expected_tool,
            args: operation.clone(),
            approval_status: crate::AgentApprovalStatus::Approved,
            reason,
        };
        let expected_result_item = crate::conversation_trace::projected_tool_result_trace_item(
            result_sequence,
            &expected_call,
            &audited_tool_result,
        );
        if durable_trace.items[result_index] != expected_result_item {
            return Err(
                "durable trace ToolResult payload differs from the terminal audit".to_string(),
            );
        }

        // A continuation may already have appended later model/tool activity. Settlement proof is
        // evaluated at the exact ToolResult boundary, while the full durable trace was validated
        // above as an append-only trace.
        let mut settlement_trace = durable_trace.clone();
        settlement_trace.items.truncate(result_index + 1);
        settlement_trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::InProgress;
        settlement_trace.terminal_error = None;

        let completed_at = audit
            .completed_at
            .ok_or_else(|| "manual file-effect terminal audit lacks completed_at".to_string())?;
        let mut validation_pending = pending.clone();
        // The terminal lifecycle transition scrubs the original pre-terminal label. `approved` is
        // valid for every current manual file-effect action and does not weaken frozen identity.
        validation_pending.status = "approved".to_string();
        validate_manual_file_effect_settlement_request(
            &audit,
            "approved",
            target_status,
            &settlement_trace,
            completed_at,
        )?;
        validate_manual_file_effect_settlement_identity(
            &validation_pending,
            &audit,
            "approved",
            target_status,
            &settlement_trace,
        )
    })();
    Ok(proof.is_ok())
}

/// Releases only the active-process fence for an interrupted approved command.
///
/// The pending target and pre-terminal audit remain available for diagnosis. A matching resolved
/// Session proves that the exact Host process can no longer mutate files, but does not fabricate a
/// terminal ToolResult or claim success. `outcome_unknown` is intentionally excluded because its
/// process/file-effect outcome is still unresolved.
fn manual_command_has_resolved_terminal_session(
    connection: &rusqlite::Connection,
    pending: &AgentPendingActionRecord,
) -> Result<bool, String> {
    if pending.tool_name != "run_command"
        || !matches!(
            pending.target_status.as_deref(),
            Some("completed" | "failed" | "cancelled")
        )
    {
        return Ok(false);
    }
    let (Some(conversation_id), Some(assistant_message_id), Some(call_id)) = (
        pending.conversation_id.as_deref(),
        pending.assistant_message_id.as_deref(),
        pending.tool_call_id.as_deref(),
    ) else {
        return Ok(false);
    };
    let command = match serde_json::from_str::<AgentProposedAction>(&pending.action_json) {
        Ok(AgentProposedAction::Command { command }) if command.id == call_id => command.command,
        _ => return Ok(false),
    };
    agent_command_session_repository::has_resolved_terminal_session_for_action(
        connection,
        conversation_id,
        assistant_message_id,
        &pending.run_id,
        call_id,
        &command,
    )
    .map_err(storage_error)
}

fn validated_mcp_approval_for_durable_call(
    record: &AgentPendingActionRecord,
    durable: &DurablePendingTraceSnapshot,
) -> Option<crate::AgentMcpToolApproval> {
    let action = serde_json::from_str::<crate::AgentProposedAction>(&record.action_json).ok()?;
    let crate::AgentProposedAction::McpToolCall { approval } = action else {
        return None;
    };
    let identity = &approval.identity;
    let provenance = &identity.provenance;
    let crate::AgentToolIdentity::Mcp {
        provenance: durable_provenance,
    } = &durable.provenance
    else {
        return None;
    };
    let expected_storage_id = format!(
        "v2:{}:{}:{}",
        identity.run_id.len(),
        identity.run_id,
        identity.action_id
    );
    (record.action_id == expected_storage_id
        && identity.run_id == record.run_id
        && identity.call_id == durable.call.id
        && record.tool_call_id.as_deref() == Some(identity.call_id.as_str())
        && record.tool_name == provenance.model_tool_name
        && approval.call.id == identity.call_id
        && approval.call.tool == provenance.model_tool_name
        && approval.summary.server_id == provenance.server_id
        && approval.summary.scope == provenance.scope
        && approval.summary.raw_tool_name == provenance.raw_tool_name
        && approval.summary.model_tool_name == provenance.model_tool_name
        && *durable_provenance == *provenance
        && approval.summary.external)
        .then_some(*approval)
}

fn terminalize_mcp_action_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    request: &McpActionTerminalizationRequest,
    updated_at: i64,
) -> Result<Option<AgentPendingActionRecord>, String> {
    if !valid_mcp_terminal_transition(&request.expected_status, request.outcome) {
        return Err("invalid MCP terminal transition".to_string());
    }
    let Some(record) =
        pending_action_repository::load_pending_action(transaction, &request.action_id)
            .map_err(storage_error)?
    else {
        return Ok(None);
    };
    if record.status != request.expected_status || record.target_status.is_some() {
        return Ok(None);
    }
    if record.action_type != "mcp_tool_call" {
        return Err("MCP terminalization rejected a non-MCP action".to_string());
    }
    let durable = load_durable_pending_trace_snapshot(transaction, &record, true)?;
    if !matches!(durable.provenance, crate::AgentToolIdentity::Mcp { .. }) {
        return Err("MCP terminalization requires durable MCP Tool provenance".to_string());
    }
    // A malformed or drifted public action must still be safely retired and scrubbed. Only a
    // fully validated typed action is allowed to update the Renderer-safe MCP lifecycle.
    let approval = validated_mcp_approval_for_durable_call(&record, &durable);
    let conversation_id = record
        .conversation_id
        .as_deref()
        .ok_or_else(|| "MCP terminalization requires a conversation owner".to_string())?;
    let assistant_message_id = record
        .assistant_message_id
        .as_deref()
        .ok_or_else(|| "MCP terminalization requires an Assistant owner".to_string())?;
    let rejection_status = matches!(
        request.outcome,
        McpStartupActionTerminalOutcome::Rejected | McpStartupActionTerminalOutcome::Cancelled
    )
    .then_some(crate::AgentApprovalStatus::Rejected);
    let terminal_trace_status = if request.outcome == McpStartupActionTerminalOutcome::Cancelled {
        crate::ConversationTurnTraceTerminalStatus::Cancelled
    } else {
        crate::ConversationTurnTraceTerminalStatus::Failed
    };
    let terminal =
        crate::conversation_trace::terminal_conversation_trace_from_snapshot_with_tool_approval(
            durable.snapshot,
            &record.run_id,
            conversation_id,
            assistant_message_id,
            terminal_trace_status,
            request.outcome.safe_reason(),
            rejection_status,
        )?;

    let terminal_status = request.outcome.pending_status();
    let run_status = request.outcome.run_status();
    let affected = transaction
        .execute(
            "
            UPDATE agent_pending_actions
            SET status = ?3,
                target_status = ?3,
                action_json = '{}',
                agent_input_json = '{}',
                updated_at = ?4
            WHERE action_id = ?1
              AND status = ?2
              AND target_status IS NULL
              AND action_type = 'mcp_tool_call'
            ",
            rusqlite::params![
                request.action_id,
                request.expected_status,
                terminal_status,
                updated_at
            ],
        )
        .map_err(storage_error)?;
    if affected != 1 {
        return Err("MCP terminalization lost its status CAS".to_string());
    }

    transaction
        .execute(
            "
            UPDATE agent_action_audit
            SET status = ?2,
                action_json = '{}',
                patch_result_json = NULL,
                command_result_json = NULL,
                tool_result_json = NULL,
                error = ?3,
                blocked_reason = ?4,
                completed_at = COALESCE(completed_at, ?5)
            WHERE action_id = ?1
            ",
            rusqlite::params![
                request.action_id,
                terminal_status,
                request.outcome.error_code(),
                request.outcome.safe_reason(),
                updated_at
            ],
        )
        .map_err(storage_error)?;
    transaction
        .execute(
            "DELETE FROM mcp_approval_payload_envelopes WHERE action_id = ?1",
            [&request.action_id],
        )
        .map_err(storage_error)?;

    chat_repository::update_message_run_terminal_state(
        transaction,
        conversation_id,
        assistant_message_id,
        &record.run_id,
        Some(request.outcome.message_status()),
        run_status,
        updated_at,
    )
    .map_err(storage_error)?;
    if let Some(approval) = approval.as_ref() {
        let identity = &approval.identity;
        let provenance = &identity.provenance;
        chat_repository::update_message_mcp_invocation_terminal_state(
            transaction,
            conversation_id,
            assistant_message_id,
            &chat_repository::McpInvocationTerminalProjection {
                action_id: &identity.action_id,
                invocation_id: &identity.invocation_id,
                call_id: &identity.call_id,
                server_id: &provenance.server_id,
                server_display_name: &approval.summary.server_display_name,
                scope: &provenance.scope,
                raw_tool_name: &provenance.raw_tool_name,
                model_tool_name: &provenance.model_tool_name,
                state: request.outcome.invocation_state(),
                dispatch_certainty: request.outcome.dispatch_certainty(),
                outcome: request.outcome.invocation_outcome(),
                is_error: request.outcome.invocation_is_error(),
                error_code: request.outcome.error_code(),
            },
        )
        .map_err(storage_error)?;
    }
    conversation_trace_repository::commit_trace_in_connection(
        transaction,
        &terminal.trace,
        record.created_at,
        updated_at,
    )
    .map_err(storage_error)?;
    conversation_model_context_repository::commit_items_in_connection(
        transaction,
        conversation_id,
        assistant_message_id,
        &terminal.model_context_items,
    )
    .map_err(storage_error)?;
    transaction
        .execute(
            "
            UPDATE agent_usage_records
            SET status = ?2, error = ?3, completed_at = ?4
            WHERE run_id = ?1
              AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')
            ",
            rusqlite::params![
                record.run_id,
                run_status,
                request.outcome.error_code(),
                updated_at
            ],
        )
        .map_err(storage_error)?;
    Ok(Some(record))
}

impl StorageService {
    /// Settles the hidden dispatch journal for one automatically authorized MCP invocation.
    ///
    /// `approved` is a definitely-not-dispatched preparation state. `executing` is the durable
    /// boundary after which startup recovery must assume the external Tool may have run. This
    /// method changes only the internal journal; the live Agent runtime remains responsible for
    /// its normal ToolResult/trace continuation.
    pub fn settle_auto_mcp_action_journal(
        &self,
        action_id: &str,
        expected_status: &str,
        outcome: McpAutoActionJournalTerminalOutcome,
        updated_at: i64,
    ) -> Result<bool, String> {
        let transition_is_valid = match expected_status {
            "approved" => matches!(
                outcome,
                McpAutoActionJournalTerminalOutcome::Failed
                    | McpAutoActionJournalTerminalOutcome::Cancelled
            ),
            "executing" => true,
            _ => false,
        };
        if !transition_is_valid {
            return Err("invalid automatic MCP journal terminal transition".to_string());
        }

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let Some(record) = pending_action_repository::load_pending_action(&transaction, action_id)
            .map_err(storage_error)?
        else {
            return Ok(false);
        };
        if record.status != expected_status || record.action_type != "mcp_tool_call" {
            return Ok(false);
        }
        let action = serde_json::from_str::<AgentProposedAction>(&record.action_json)
            .map_err(|_| "automatic MCP journal contains an invalid frozen action".to_string())?;
        let AgentProposedAction::McpToolCall { approval } = action else {
            return Err("automatic MCP journal action type is inconsistent".to_string());
        };
        let identity = &approval.identity;
        let provenance = &identity.provenance;
        let expected_storage_id = format!(
            "v2:{}:{}:{}",
            identity.run_id.len(),
            identity.run_id,
            identity.action_id
        );
        if approval.approval_mode != crate::AgentMcpApprovalMode::Auto
            || approval.call.approval_status != crate::AgentApprovalStatus::Approved
            || record.action_id != expected_storage_id
            || record.run_id != identity.run_id
            || record.tool_call_id.as_deref() != Some(identity.call_id.as_str())
            || record.tool_name != provenance.model_tool_name
            || approval.call.id != identity.call_id
            || approval.call.tool != provenance.model_tool_name
            || approval.summary.server_id != provenance.server_id
            || approval.summary.scope != provenance.scope
            || approval.summary.raw_tool_name != provenance.raw_tool_name
            || approval.summary.model_tool_name != provenance.model_tool_name
            || !approval.summary.external
        {
            return Err("automatic MCP journal rejected a drifted typed identity".to_string());
        }

        let terminal_status = outcome.pending_status();
        let affected = transaction
            .execute(
                "
                UPDATE agent_pending_actions
                SET status = ?3,
                    target_status = ?3,
                    action_json = '{}',
                    agent_input_json = '{}',
                    updated_at = ?4
                WHERE action_id = ?1
                  AND status = ?2
                  AND action_type = 'mcp_tool_call'
                ",
                rusqlite::params![action_id, expected_status, terminal_status, updated_at],
            )
            .map_err(storage_error)?;
        if affected != 1 {
            return Ok(false);
        }
        transaction
            .execute(
                "
                UPDATE agent_action_audit
                SET status = ?2,
                    action_json = '{}',
                    patch_result_json = NULL,
                    command_result_json = NULL,
                    tool_result_json = NULL,
                    error = ?3,
                    blocked_reason = ?4,
                    completed_at = COALESCE(completed_at, ?5)
                WHERE action_id = ?1
                ",
                rusqlite::params![
                    action_id,
                    terminal_status,
                    outcome.error_code(),
                    outcome.safe_reason(),
                    updated_at
                ],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM mcp_approval_payload_envelopes WHERE action_id = ?1",
                [action_id],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(true)
    }

    /// Atomically terminalizes one startup MCP action without retaining action arguments.
    ///
    /// `pending` and `approved` are pre-dispatch states and therefore accept only definitely-not-
    /// dispatched outcomes. `executing` is the durable dispatch boundary and can only become
    /// outcome-unknown. The pending row, matching audit, owner run state, usage, and any durable
    /// ciphertext envelope are settled in one SQLite transaction.
    pub fn terminalize_mcp_agent_action_on_startup(
        &self,
        action_id: &str,
        expected_status: &str,
        outcome: McpStartupActionTerminalOutcome,
        updated_at: i64,
    ) -> Result<bool, String> {
        if !valid_mcp_terminal_transition(expected_status, outcome) {
            return Err("invalid MCP startup terminal transition".to_string());
        }

        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let request = McpActionTerminalizationRequest {
            action_id: action_id.to_string(),
            expected_status: expected_status.to_string(),
            outcome,
        };
        let changed =
            terminalize_mcp_action_in_transaction(&transaction, &request, updated_at)?.is_some();
        transaction.commit().map_err(storage_error)?;
        Ok(changed)
    }

    /// Atomically terminalizes a Host-selected set of MCP approvals.
    ///
    /// The entire batch commits or rolls back together. Pre-dispatch rows may become unavailable,
    /// expired, policy-denied, rejected, or cancelled; an `executing` row can only become outcome-
    /// unknown. Every durable payload envelope and argument-bearing pending/audit projection is
    /// scrubbed in the same SQLite transaction.
    pub fn terminalize_mcp_agent_actions(
        &self,
        requests: &[McpActionTerminalizationRequest],
        updated_at: i64,
    ) -> Result<usize, String> {
        if requests.is_empty() {
            return Ok(0);
        }
        let mut unique_ids = HashSet::with_capacity(requests.len());
        if requests.iter().any(|request| {
            !unique_ids.insert(request.action_id.as_str())
                || !valid_mcp_terminal_transition(&request.expected_status, request.outcome)
        }) {
            return Err("invalid or duplicate MCP terminalization request".to_string());
        }

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        for request in requests {
            if terminalize_mcp_action_in_transaction(&transaction, request, updated_at)?.is_none() {
                return Err("MCP terminalization batch lost an expected status CAS".to_string());
            }
        }
        transaction.commit().map_err(storage_error)?;
        Ok(requests.len())
    }

    /// Safely retires one current pending row whose private action/resume projection cannot be
    /// decoded or authenticated at startup.
    ///
    /// The durable trace and staged Provider/Runtime ToolCall identity remain the sole execution
    /// authority. `pending`/`approved` rows are known not to have dispatched; `executing` rows are
    /// conservatively classified outcome-unknown. No Tool is invoked by this path.
    pub fn retire_unsupported_or_malformed_pending_agent_action_on_startup(
        &self,
        action_id: &str,
        expected_status: &str,
        updated_at: i64,
    ) -> Result<bool, String> {
        let (error_code, reason) = match expected_status {
            "pending" | "approved" => (
                "agent.pending_action_unsupported_or_malformed",
                "The pending action could not be safely restored and was retired before dispatch.",
            ),
            "executing" => (
                "agent.pending_action_outcome_unknown",
                "The pending action crossed its durable dispatch boundary, but its outcome is unknown and it was not replayed.",
            ),
            _ => return Err("invalid malformed pending-action retirement status".to_string()),
        };
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let Some(record) = pending_action_repository::load_pending_action(&transaction, action_id)
            .map_err(storage_error)?
        else {
            return Ok(false);
        };
        if record.status != expected_status || record.target_status.is_some() {
            return Ok(false);
        }
        let durable = load_durable_pending_trace_snapshot(
            &transaction,
            &record,
            expected_status != "executing",
        )?;
        let conversation_id = record
            .conversation_id
            .as_deref()
            .ok_or_else(|| "malformed pending action requires a conversation owner".to_string())?;
        let assistant_message_id = record
            .assistant_message_id
            .as_deref()
            .ok_or_else(|| "malformed pending action requires an Assistant owner".to_string())?;
        let terminal = crate::terminal_conversation_trace_from_snapshot(
            durable.snapshot,
            &record.run_id,
            conversation_id,
            assistant_message_id,
            crate::ConversationTurnTraceTerminalStatus::Failed,
            reason,
        )?;

        let affected = transaction
            .execute(
                "
                UPDATE agent_pending_actions
                SET status = 'failed',
                    target_status = 'failed',
                    action_json = '{}',
                    agent_input_json = '{}',
                    updated_at = ?3
                WHERE action_id = ?1
                  AND status = ?2
                  AND target_status IS NULL
                ",
                rusqlite::params![action_id, expected_status, updated_at],
            )
            .map_err(storage_error)?;
        if affected != 1 {
            return Ok(false);
        }
        transaction
            .execute(
                "
                UPDATE agent_action_audit
                SET status = 'failed',
                    action_json = '{}',
                    patch_result_json = NULL,
                    command_result_json = NULL,
                    tool_result_json = NULL,
                    error = ?2,
                    blocked_reason = ?3,
                    completed_at = COALESCE(completed_at, ?4)
                WHERE action_id = ?1
                ",
                rusqlite::params![action_id, error_code, reason, updated_at],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM mcp_approval_payload_envelopes WHERE action_id = ?1",
                [action_id],
            )
            .map_err(storage_error)?;
        chat_repository::update_message_run_terminal_state(
            &transaction,
            conversation_id,
            assistant_message_id,
            &record.run_id,
            Some("error"),
            "failed",
            updated_at,
        )
        .map_err(storage_error)?;
        conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            &terminal.trace,
            record.created_at,
            updated_at,
        )
        .map_err(storage_error)?;
        conversation_model_context_repository::commit_items_in_connection(
            &transaction,
            conversation_id,
            assistant_message_id,
            &terminal.model_context_items,
        )
        .map_err(storage_error)?;
        transaction
            .execute(
                "
                UPDATE agent_usage_records
                SET status = 'failed', error = ?2, completed_at = ?3
                WHERE run_id = ?1
                  AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')
                ",
                rusqlite::params![record.run_id, error_code, updated_at],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(true)
    }

    pub fn list_unsettled_file_effects(&self) -> Result<Vec<AgentUnsettledFileEffect>, String> {
        let connection = self.state.connection()?;
        let candidates = agent_action_audit_repository::list_unsettled_file_effects(&connection)
            .map_err(storage_error)?;
        let mut unsettled = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let pending =
                pending_action_repository::load_pending_action(&connection, &candidate.action_id)
                    .map_err(storage_error)?;
            let is_authoritatively_settled = match pending.as_ref() {
                Some(pending) => {
                    manual_file_effect_has_authoritative_settlement(&connection, pending)?
                }
                None => false,
            };
            let command_process_is_known_stopped = match pending.as_ref() {
                Some(pending) => {
                    manual_command_has_resolved_terminal_session(&connection, pending)?
                }
                None => false,
            };
            if !is_authoritatively_settled && !command_process_is_known_stopped {
                unsettled.push(candidate);
            }
        }
        Ok(unsettled)
    }

    pub fn inspect_agent_action_audit_execution(
        &self,
        record: AgentActionAuditRecord,
    ) -> Result<Option<agent_action_audit_repository::AgentActionAuditExecutionClaimOutcome>, String>
    {
        let connection = self.state.connection()?;
        agent_action_audit_repository::inspect_action_audit_execution(&connection, &record)
            .map_err(storage_error)
    }

    pub fn insert_agent_action_audit_if_absent(
        &self,
        record: AgentActionAuditRecord,
    ) -> Result<bool, String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::insert_action_audit_record_if_absent(&connection, &record)
            .map_err(storage_error)
    }

    pub fn claim_agent_action_audit_execution(
        &self,
        record: AgentActionAuditRecord,
    ) -> Result<agent_action_audit_repository::AgentActionAuditExecutionClaimOutcome, String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::claim_action_audit_execution(&connection, &record)
            .map_err(storage_error)
    }

    pub fn finalize_agent_action_audit_execution(
        &self,
        record: AgentActionAuditRecord,
    ) -> Result<agent_action_audit_repository::AgentActionAuditFinalizationOutcome, String> {
        if !matches!(record.status.as_str(), "completed" | "failed") {
            return Err(format!(
                "自动操作审计终态无效：{}（仅允许 completed 或 failed）。",
                record.status
            ));
        }
        if record.completed_at.is_none() {
            return Err("自动操作审计终态缺少 completed_at。".to_string());
        }
        let connection = self.state.connection()?;
        agent_action_audit_repository::finalize_claimed_action_audit_execution(&connection, &record)
            .map_err(storage_error)
    }

    pub fn upsert_agent_action_audit(&self, record: AgentActionAuditRecord) -> Result<(), String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::upsert_action_audit_record(&connection, &record)
            .map_err(storage_error)
    }

    pub fn store_pending_agent_action(
        &self,
        record: AgentPendingActionRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        let connection = self.state.connection()?;
        let outcome = pending_action_repository::store_pending_action(&connection, &record)
            .map_err(storage_error)?;
        match outcome {
            pending_action_repository::PendingActionStoreOutcome::Conflict {
                ref existing_run_id,
                ref existing_status,
            } => Err(format!(
                "待审批操作 actionId={} 已属于 runId={}（status={}）；拒绝覆盖冻结快照。",
                record.action_id, existing_run_id, existing_status
            )),
            _ => Ok(outcome),
        }
    }

    pub fn list_pending_agent_actions(&self) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_pending_actions(&connection).map_err(storage_error)
    }

    /// Loads one frozen approval fact by its backend-framed id. Renderer-facing callers must use
    /// a higher-level root projection and must never receive `agent_input_json`.
    pub fn get_pending_agent_action(
        &self,
        approval_id: &str,
    ) -> Result<Option<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::load_pending_action(&connection, approval_id)
            .map_err(storage_error)
    }

    /// Returns the current-format rows owned by pending or MCP-specific startup recovery.
    pub fn list_recoverable_agent_actions_after_reconciliation(
        &self,
    ) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_recoverable_actions_after_reconciliation(&connection)
            .map_err(storage_error)
    }

    pub fn reconcile_interrupted_pending_agent_actions(
        &self,
        updated_at: i64,
    ) -> Result<Vec<AgentPendingActionRecord>, String> {
        const INTERRUPTION_REASON: &str =
            "The application exited after approval; process outcome is unknown and was not replayed.";
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let interrupted = pending_action_repository::list_interrupted_actions(&transaction)
            .map_err(storage_error)?;
        let pending_successors =
            pending_action_repository::list_pending_actions(&transaction).map_err(storage_error)?;
        let mut retired_successors = HashSet::new();
        let mut claimed_successors = HashSet::new();
        for record in &interrupted {
            let durable_run_status = match (
                record.conversation_id.as_deref(),
                record.assistant_message_id.as_deref(),
            ) {
                (Some(conversation_id), Some(message_id)) => {
                    let raw = transaction
                        .query_row(
                            "SELECT agent_run_json FROM messages
                         WHERE conversation_id = ?1 AND id = ?2",
                            rusqlite::params![conversation_id, message_id],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .optional()
                        .map_err(storage_error)?
                        .flatten();
                    raw.map(|raw| {
                        serde_json::from_str::<serde_json::Value>(&raw)
                            .map_err(|error| {
                                format!(
                                    "无法解析中断操作 {} 的 agent run 状态：{error}",
                                    record.action_id
                                )
                            })
                            .map(|run| {
                                run.get("status")
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToString::to_string)
                            })
                    })
                    .transpose()?
                    .flatten()
                }
                _ => None,
            };
            let reconciled_status = match record.target_status.as_deref() {
                Some(status @ ("completed" | "failed" | "rejected" | "cancelled")) => status,
                None => {
                    let affected = pending_action_repository::set_pending_action_target_status(
                        &transaction,
                        &record.action_id,
                        &record.status,
                        "failed",
                        updated_at,
                    )
                    .map_err(storage_error)?;
                    if affected != 1 {
                        return Err(format!(
                            "启动对账无法为中断的待审批操作 {} 写入 failed 目标终态。",
                            record.action_id
                        ));
                    }
                    "failed"
                }
                Some(status) => {
                    return Err(format!(
                        "启动对账发现待审批操作 {} 的目标终态无效：{status}",
                        record.action_id
                    ));
                }
            };
            let affected = pending_action_repository::transition_pending_action(
                &transaction,
                &record.action_id,
                &record.status,
                reconciled_status,
                "{}",
                updated_at,
            )
            .map_err(storage_error)?;
            if affected != 1 {
                return Err(format!(
                    "启动对账无法以 CAS 迁移待审批操作 {}（expectedStatus={}）。",
                    record.action_id, record.status
                ));
            }
            let successor_candidates = pending_successors
                .iter()
                .filter(|candidate| {
                    !retired_successors.contains(&candidate.action_id)
                        && is_pending_successor_candidate(record, candidate)
                })
                .collect::<Vec<_>>();
            let mut valid_successors = Vec::new();
            for candidate in successor_candidates.iter().copied() {
                if is_valid_pending_successor(&transaction, record, candidate)? {
                    valid_successors.push(candidate);
                }
            }
            if valid_successors.len() > 1 {
                return Err(format!(
                    "启动对账发现 action {} 存在多个合法待审批后继。",
                    record.action_id
                ));
            }
            for candidate in successor_candidates.iter().copied().filter(|candidate| {
                !valid_successors
                    .iter()
                    .any(|valid| valid.action_id == candidate.action_id)
            }) {
                let target_affected = pending_action_repository::set_pending_action_target_status(
                    &transaction,
                    &candidate.action_id,
                    "pending",
                    "cancelled",
                    updated_at,
                )
                .map_err(storage_error)?;
                if target_affected != 1 {
                    return Err(format!(
                        "启动对账无法取消无效待审批后继 {}。",
                        candidate.action_id
                    ));
                }
                let transition_affected = pending_action_repository::transition_pending_action(
                    &transaction,
                    &candidate.action_id,
                    "pending",
                    "cancelled",
                    "{}",
                    updated_at,
                )
                .map_err(storage_error)?;
                if transition_affected != 1 {
                    return Err(format!(
                        "启动对账无法终结无效待审批后继 {}。",
                        candidate.action_id
                    ));
                }
                retired_successors.insert(candidate.action_id.clone());
            }
            let valid_successor = valid_successors.first().copied();
            if let Some(successor) = valid_successor {
                if !claimed_successors.insert(successor.action_id.clone()) {
                    return Err(format!(
                        "启动对账发现待审批后继 {} 被多个父操作声明。",
                        successor.action_id
                    ));
                }
                if matches!(
                    durable_run_status.as_deref(),
                    Some("completed" | "failed" | "cancelled")
                ) {
                    return Err(format!(
                        "启动对账发现 run {} 同时存在终态 assistant 与合法待审批后继。",
                        record.run_id
                    ));
                }
                if let (Some(conversation_id), Some(message_id)) = (
                    record.conversation_id.as_deref(),
                    record.assistant_message_id.as_deref(),
                ) {
                    chat_repository::update_message_run_waiting_state(
                        &transaction,
                        conversation_id,
                        message_id,
                        &record.run_id,
                        updated_at,
                    )
                    .map_err(storage_error)?;
                }
                transaction
                    .execute(
                        "UPDATE agent_usage_records
                         SET status = 'waiting_for_approval', error = NULL, completed_at = NULL
                         WHERE run_id = ?1
                           AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')",
                        [&record.run_id],
                    )
                    .map_err(storage_error)?;
                continue;
            }
            if matches!(
                durable_run_status.as_deref(),
                Some("completed" | "failed" | "cancelled")
            ) {
                let assistant_message_id =
                    record.assistant_message_id.as_deref().ok_or_else(|| {
                        format!(
                            "启动对账发现终态 run {} 缺少 Assistant owner。",
                            record.run_id
                        )
                    })?;
                let trace = conversation_trace_repository::get_trace_for_message(
                    &transaction,
                    assistant_message_id,
                )
                .map_err(storage_error)?
                .ok_or_else(|| {
                    format!(
                        "启动对账发现终态 run {} 缺少 ConversationTurnTrace。",
                        record.run_id
                    )
                })?;
                let model_context_items =
                    conversation_model_context_repository::get_log_for_message(
                        &transaction,
                        assistant_message_id,
                    )
                    .map_err(storage_error)?
                    .map(|log| log.items)
                    .unwrap_or_default();
                trace
                    .validate_complete_model_context(&model_context_items)
                    .map_err(|error| {
                        format!(
                            "启动对账发现终态 run {} 的模型历史不完整：{error}",
                            record.run_id
                        )
                    })?;
                transaction
                    .execute(
                        "UPDATE agent_usage_records
                         SET status = ?2, completed_at = COALESCE(completed_at, ?3)
                         WHERE run_id = ?1
                           AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')",
                        rusqlite::params![record.run_id, durable_run_status, updated_at],
                    )
                    .map_err(storage_error)?;
                continue;
            }
            {
                let conversation_id = record.conversation_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账无法终结 run {}：缺少 conversation owner。",
                        record.run_id
                    )
                })?;
                let message_id = record.assistant_message_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账无法终结 run {}：缺少 Assistant owner。",
                        record.run_id
                    )
                })?;
                let durable = load_durable_pending_trace_snapshot(&transaction, record, false)
                    .map_err(|error| {
                        format!(
                            "启动对账无法终结 run {} 的 durable ToolCall：{error}",
                            record.run_id
                        )
                    })?;
                let terminal = crate::terminal_conversation_trace_from_snapshot(
                    durable.snapshot,
                    &record.run_id,
                    conversation_id,
                    message_id,
                    crate::ConversationTurnTraceTerminalStatus::Failed,
                    INTERRUPTION_REASON,
                )?;
                conversation_trace_repository::commit_trace_in_connection(
                    &transaction,
                    &terminal.trace,
                    record.created_at,
                    updated_at,
                )
                .map_err(storage_error)?;
                conversation_model_context_repository::commit_items_in_connection(
                    &transaction,
                    conversation_id,
                    message_id,
                    &terminal.model_context_items,
                )
                .map_err(storage_error)?;
                chat_repository::update_message_run_terminal_state(
                    &transaction,
                    conversation_id,
                    message_id,
                    &record.run_id,
                    Some("error"),
                    "failed",
                    updated_at,
                )
                .map_err(storage_error)?;
            }
            transaction
                .execute(
                    "UPDATE agent_usage_records
                     SET status = 'failed', error = ?2, completed_at = ?3
                     WHERE run_id = ?1
                       AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')",
                    rusqlite::params![record.run_id, INTERRUPTION_REASON, updated_at],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(interrupted)
    }

    pub fn transition_pending_agent_action(
        &self,
        action_id: &str,
        expected_status: &str,
        status: &str,
        agent_input_json: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        let affected = pending_action_repository::transition_pending_action(
            &connection,
            action_id,
            expected_status,
            status,
            agent_input_json,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作状态迁移必须且只能更新一条记录，actionId={action_id}，实际更新 {affected} 条。"
            ));
        }
        Ok(())
    }

    pub fn set_pending_agent_action_target_status(
        &self,
        action_id: &str,
        expected_status: &str,
        target_status: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        let affected = pending_action_repository::set_pending_action_target_status(
            &connection,
            action_id,
            expected_status,
            target_status,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作目标终态写入必须且只能更新一条记录，actionId={action_id}，expectedStatus={expected_status}，实际更新 {affected} 条。"
            ));
        }
        Ok(())
    }

    /// Atomically retires a claimed action when its model continuation cannot reach Runtime.
    ///
    /// The action executor has already persisted `expected_target_status` before this boundary.
    /// The exact target is therefore part of the CAS: a stale continuation cannot overwrite a
    /// newer decision. Pending lifecycle, assistant/run terminal state, trace/model context and
    /// usage become visible together or remain entirely unchanged for recovery.
    #[allow(clippy::too_many_arguments)]
    pub fn fail_claimed_agent_action_continuation(
        &self,
        action_id: &str,
        expected_status: &str,
        expected_target_status: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        failure_message: &str,
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
        completed_at: i64,
        usage: Option<&AgentUsageRecordInsert>,
    ) -> Result<(), String> {
        if !matches!(expected_status, "approved" | "executing" | "rejected") {
            return Err(format!(
                "pre-Runtime continuation failure requires approved/executing/rejected status, got {expected_status}"
            ));
        }
        if !matches!(
            expected_target_status,
            "rejected" | "cancelled" | "completed" | "failed"
        ) {
            return Err(format!(
                "pre-Runtime continuation failure requires a terminal target, got {expected_target_status}"
            ));
        }
        if trace.conversation_id != conversation_id
            || trace.assistant_message_id != assistant_message_id
            || trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::Failed
            || trace.terminal_error.as_deref() != Some(failure_message)
        {
            return Err(
                "pre-Runtime continuation failure trace identity, status or error is invalid"
                    .to_string(),
            );
        }
        trace
            .validate_complete_model_context(model_context_items)
            .map_err(|error| {
                format!("pre-Runtime continuation model context is invalid: {error}")
            })?;
        if let Some(usage) = usage {
            if usage.run_id != trace.run_id
                || usage.conversation_id != conversation_id
                || usage.message_id != assistant_message_id
                || usage.status.as_deref() != Some("failed")
                || usage.error.as_deref() != Some(failure_message)
            {
                return Err(
                    "pre-Runtime continuation usage does not match the terminal Turn failure"
                        .to_string(),
                );
            }
        }

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let affected = pending_action_repository::fail_claimed_action_continuation(
            &transaction,
            action_id,
            &trace.run_id,
            conversation_id,
            assistant_message_id,
            expected_status,
            expected_target_status,
            completed_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "pre-Runtime continuation failure lost its pending-action CAS: actionId={action_id}, expectedStatus={expected_status}, expectedTargetStatus={expected_target_status}"
            ));
        }
        chat_repository::update_message_status_and_content(
            &transaction,
            conversation_id,
            assistant_message_id,
            failure_message,
            Some("error"),
            completed_at,
        )
        .map_err(storage_error)?;
        chat_repository::update_message_run_terminal_state(
            &transaction,
            conversation_id,
            assistant_message_id,
            &trace.run_id,
            Some("error"),
            "failed",
            completed_at,
        )
        .map_err(storage_error)?;
        conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            trace,
            completed_at,
            completed_at,
        )
        .map_err(storage_error)?;
        conversation_model_context_repository::commit_items_in_connection(
            &transaction,
            conversation_id,
            assistant_message_id,
            model_context_items,
        )
        .map_err(storage_error)?;
        let durable_model_context_items =
            conversation_model_context_repository::get_log_for_message(
                &transaction,
                assistant_message_id,
            )
            .map_err(storage_error)?
            .map(|log| log.items)
            .unwrap_or_default();
        trace
            .validate_complete_model_context(&durable_model_context_items)
            .map_err(|error| format!("terminal Assistant model context is incomplete: {error}"))?;
        if let Some(usage) = usage {
            usage_repository::upsert_usage_record(&transaction, usage).map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }

    /// Atomically records a pending action's durable outcome and its paired in-progress trace.
    ///
    /// A tool result must not become a terminal pending-action fact without also closing the
    /// corresponding tool call in the conversation trace. Keeping both writes in one SQLite
    /// transaction gives approval continuations a single recovery boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_pending_agent_action_result_trace(
        &self,
        action_id: &str,
        expected_status: &str,
        target_status: &str,
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        committed_at: i64,
    ) -> Result<bool, String> {
        if !matches!(
            target_status,
            "rejected" | "cancelled" | "completed" | "failed"
        ) {
            return Err(format!("待审批操作目标状态必须是终态：{target_status}"));
        }
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let affected = pending_action_repository::set_pending_action_target_status(
            &transaction,
            action_id,
            expected_status,
            target_status,
            committed_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作目标状态写入冲突：actionId={action_id}, expectedStatus={expected_status}, targetStatus={target_status}"
            ));
        }
        let trace_changed = conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            trace,
            trace_created_at,
            committed_at,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(trace_changed)
    }

    /// Atomically settles a manually approved file-producing action and its exact bounded model
    /// projection across every durable representation.
    ///
    /// The action audit, pending-action target, paired ToolResult trace, and model projection form
    /// one recovery boundary. Retrying the exact same bundle is idempotent; a different identity
    /// or terminal result fails closed without changing any durable representation.
    pub fn commit_pending_agent_action_audited_result_trace_with_model_context(
        &self,
        terminal_audit: &AgentActionAuditRecord,
        expected_pending_status: &str,
        target_status: &str,
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
        committed_at: i64,
    ) -> Result<AgentPendingActionResultCommitOutcome, String> {
        validate_manual_file_effect_settlement_request(
            terminal_audit,
            expected_pending_status,
            target_status,
            trace,
            committed_at,
        )?;
        crate::conversation_trace::validate_model_context_prefix(trace, model_context_items)
            .map_err(|error| format!("manual file-effect model context is invalid: {error}"))?;

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let pending =
            pending_action_repository::load_pending_action(&transaction, &terminal_audit.action_id)
                .map_err(storage_error)?
                .ok_or_else(|| {
                    format!(
                        "manual file-effect settlement has no pending action: {}",
                        terminal_audit.action_id
                    )
                })?;
        validate_manual_file_effect_settlement_identity(
            &pending,
            terminal_audit,
            expected_pending_status,
            target_status,
            trace,
        )?;

        let audit_outcome = agent_action_audit_repository::settle_manual_terminal_action_audit(
            &transaction,
            terminal_audit,
            pending.updated_at,
        )
        .map_err(storage_error)?;
        if let agent_action_audit_repository::ManualTerminalActionAuditOutcome::Conflict {
            existing_status,
            reason,
        } = &audit_outcome
        {
            return Err(format!(
                "manual file-effect audit settlement conflict: actionId={}, existingStatus={}, reason={reason}",
                terminal_audit.action_id,
                existing_status.as_deref().unwrap_or("missing")
            ));
        }

        let target_was_already_committed = pending.target_status.as_deref() == Some(target_status);
        let affected = pending_action_repository::set_pending_action_target_status(
            &transaction,
            &pending.action_id,
            expected_pending_status,
            target_status,
            committed_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "manual file-effect pending target settlement conflict: actionId={}, expectedStatus={expected_pending_status}, targetStatus={target_status}",
                pending.action_id
            ));
        }

        let trace_changed = conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            trace,
            pending.created_at,
            committed_at,
        )
        .map_err(storage_error)?;
        let model_context_changed =
            conversation_model_context_repository::commit_items_in_connection(
                &transaction,
                &trace.conversation_id,
                &trace.assistant_message_id,
                model_context_items,
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;

        Ok(
            if matches!(
                audit_outcome,
                agent_action_audit_repository::ManualTerminalActionAuditOutcome::Idempotent
            ) && target_was_already_committed
                && !trace_changed
                && !model_context_changed
            {
                AgentPendingActionResultCommitOutcome::Idempotent
            } else {
                AgentPendingActionResultCommitOutcome::Committed { trace_changed }
            },
        )
    }

    /// Reads all durable pieces of a manual file-effect settlement from one SQLite snapshot.
    ///
    /// This is the commit-unknown recovery boundary for callers that received an error after an
    /// exact transaction attempt. It never mutates or repairs state: exact committed state is
    /// authoritative, a clean pre-commit state permits a fallback receipt, and every partial or
    /// conflicting shape fails closed as `Diverged`.
    pub fn inspect_pending_agent_action_audited_result_trace(
        &self,
        terminal_audit: &AgentActionAuditRecord,
        expected_pending_status: &str,
        target_status: &str,
        expected_trace: &ConversationTurnTrace,
        committed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        self.inspect_pending_agent_action_audited_result_trace_with_model_context(
            terminal_audit,
            expected_pending_status,
            target_status,
            expected_trace,
            &[],
            committed_at,
        )
    }

    /// Inspects the audit, pending target, trace, and exact model projection from one SQLite
    /// snapshot after a commit-unknown response.
    pub fn inspect_pending_agent_action_audited_result_trace_with_model_context(
        &self,
        terminal_audit: &AgentActionAuditRecord,
        expected_pending_status: &str,
        target_status: &str,
        expected_trace: &ConversationTurnTrace,
        expected_model_context_items: &[ConversationModelContextItem],
        committed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        validate_manual_file_effect_settlement_request(
            terminal_audit,
            expected_pending_status,
            target_status,
            expected_trace,
            committed_at,
        )?;
        crate::conversation_trace::validate_model_context_prefix(
            expected_trace,
            expected_model_context_items,
        )
        .map_err(|error| format!("manual file-effect model context is invalid: {error}"))?;

        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let Some(pending) =
            pending_action_repository::load_pending_action(&transaction, &terminal_audit.action_id)
                .map_err(storage_error)?
        else {
            return Ok(settlement_diverged(
                "pendingAction",
                "the frozen pending action is missing",
            ));
        };
        if let Err(reason) = validate_manual_file_effect_settlement_identity(
            &pending,
            terminal_audit,
            expected_pending_status,
            target_status,
            expected_trace,
        ) {
            return Ok(settlement_diverged("pendingAction", reason));
        }

        let audit = agent_action_audit_repository::load_action_audit_record(
            &transaction,
            &terminal_audit.action_id,
        )
        .map_err(storage_error)?;
        let durable_trace = conversation_trace_repository::get_trace_for_message(
            &transaction,
            &expected_trace.assistant_message_id,
        )
        .map_err(storage_error)?;
        let trace_state = manual_settlement_trace_state(durable_trace.as_ref(), expected_trace);
        let durable_model_context_items =
            conversation_model_context_repository::get_log_for_message(
                &transaction,
                &expected_trace.assistant_message_id,
            )
            .map_err(storage_error)?
            .map(|log| log.items)
            .unwrap_or_default();
        let model_context_at_or_after_boundary = expected_model_context_items.is_empty()
            || (expected_model_context_items.len() <= durable_model_context_items.len()
                && expected_model_context_items
                    == &durable_model_context_items[..expected_model_context_items.len()]);
        let model_context_before_boundary = expected_model_context_items.is_empty()
            || (durable_model_context_items.len() < expected_model_context_items.len()
                && durable_model_context_items
                    == expected_model_context_items[..durable_model_context_items.len()]);
        let target_is_committed = pending.target_status.as_deref() == Some(target_status);
        let target_is_uncommitted = pending.target_status.is_none();
        let audit_is_terminal = audit.as_ref().is_some_and(|audit| {
            agent_action_audit_repository::matches_manual_terminal_action_audit(
                audit,
                terminal_audit,
            )
        });
        let audit_is_preterminal = audit.as_ref().is_none_or(|audit| {
            agent_action_audit_repository::matches_manual_preterminal_action_audit(
                audit,
                terminal_audit,
            )
        });

        if target_is_committed && audit_is_terminal {
            if !model_context_at_or_after_boundary {
                return Ok(settlement_diverged(
                    "conversationModelContext",
                    "the audit and trace committed without their exact model projection",
                ));
            }
            return Ok(match trace_state {
                ManualSettlementTraceState::AtBoundary => {
                    AgentPendingActionSettlementInspection::CommittedAtBoundary
                }
                ManualSettlementTraceState::Advanced => {
                    AgentPendingActionSettlementInspection::CommittedAndAdvanced
                }
                ManualSettlementTraceState::Absent | ManualSettlementTraceState::BeforeBoundary => {
                    settlement_diverged(
                        "conversationTrace",
                        "the audit and target committed without the paired ToolResult boundary",
                    )
                }
                ManualSettlementTraceState::Diverged(reason) => {
                    settlement_diverged("conversationTrace", reason)
                }
            });
        }

        if target_is_uncommitted && audit_is_preterminal {
            if !model_context_before_boundary {
                return Ok(settlement_diverged(
                    "conversationModelContext",
                    "the exact model projection advanced without its audit and pending target",
                ));
            }
            return Ok(match trace_state {
                ManualSettlementTraceState::Absent | ManualSettlementTraceState::BeforeBoundary => {
                    AgentPendingActionSettlementInspection::DefinitelyUncommitted
                }
                ManualSettlementTraceState::AtBoundary | ManualSettlementTraceState::Advanced => {
                    settlement_diverged(
                        "conversationTrace",
                        "the ToolResult boundary exists without its audit and pending target",
                    )
                }
                ManualSettlementTraceState::Diverged(reason) => {
                    settlement_diverged("conversationTrace", reason)
                }
            });
        }

        if !target_is_committed && !target_is_uncommitted {
            return Ok(settlement_diverged(
                "pendingAction",
                format!(
                    "the pending action targets `{}` instead of `{target_status}`",
                    pending.target_status.as_deref().unwrap_or_default()
                ),
            ));
        }
        if audit_is_terminal {
            return Ok(settlement_diverged(
                "pendingAction",
                "the terminal audit exists without its matching pending target",
            ));
        }
        if audit_is_preterminal {
            return Ok(settlement_diverged(
                "actionAudit",
                "the pending target committed without its terminal audit",
            ));
        }
        Ok(settlement_diverged(
            "actionAudit",
            "the durable audit differs from both the frozen pre-terminal identity and the expected terminal receipt",
        ))
    }
}

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
        && durable.truncated == expected.truncated
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
    let is_mcp_action = matches!(action, AgentProposedAction::McpToolCall { .. });
    let is_mcp_rejection = is_mcp_action && target_status == "rejected";
    let valid_decision = if is_mcp_rejection {
        audit.decision.as_deref() == Some("rejected")
    } else {
        audit.decision.as_deref() == Some("approved")
    };
    if !valid_decision
        || audit.decision_source.as_deref() != Some("manual")
        || audit.patch_result_json.is_some()
    {
        return Err(
            "manual file-effect settlement contains an invalid audit lifecycle".to_string(),
        );
    }
    let (expected_action_type, expected_tool, expected_call_id, is_command) =
        manual_file_effect_identity(&action)?;
    let pending_status_is_valid = (is_mcp_rejection && expected_pending_status == "pending")
        || expected_pending_status == "approved"
        || (expected_action_type == "skill_materialization"
            && expected_pending_status == "executing")
        || (expected_action_type == "mcp_tool_call" && expected_pending_status == "executing");
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
        let validated_rejection = validate_durable_mcp_tool_result(&tool_result, target_status)?;
        if validated_rejection != is_mcp_rejection {
            return Err("MCP durable ToolResult rejection state is inconsistent".to_string());
        }
    }
    let expected_ok = if is_mcp_action {
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
    const KEYS: [&str; 6] = [
        "status",
        "sessionId",
        "output",
        "startedAt",
        "latestSequence",
        "outputTruncated",
    ];
    let session_id = result
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let session_hex = session_id.strip_prefix("cmd_");
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
        _ => Err("manual audited settlement does not support this action type".to_string()),
    }
}
