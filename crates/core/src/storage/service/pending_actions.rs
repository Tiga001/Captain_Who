use super::*;

pub use crate::storage::pending_action_repository::PendingActionJsonCommitOutcome as AgentPendingActionJsonCommitOutcome;

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
    if trace.run_id != record.run_id {
        return Err(
            "pending action durable ConversationTurnTrace run identity is inconsistent".into(),
        );
    }
    if trace.conversation_id != conversation_id {
        return Err(
            "pending action durable ConversationTurnTrace conversation identity is inconsistent"
                .into(),
        );
    }
    if trace.assistant_message_id != assistant_message_id {
        return Err(
            "pending action durable ConversationTurnTrace Assistant identity is inconsistent"
                .into(),
        );
    }
    if trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress {
        return Err("pending action durable ConversationTurnTrace is already terminal".into());
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

fn validate_manual_approval_execution_audit(
    action_id: &str,
    audit: &AgentActionAuditRecord,
    committed_at: i64,
) -> Result<(), String> {
    if action_id.trim().is_empty() || action_id != audit.action_id {
        return Err("manual approval audit action identity is invalid".to_string());
    }
    if audit.decision.as_deref() != Some("approved")
        || audit.status != "approved"
        || audit.decision_source.as_deref() != Some("manual")
        || audit.decided_at.is_none()
        || audit
            .decided_at
            .is_some_and(|decided_at| decided_at > committed_at)
        || audit.completed_at.is_some()
        || audit.file_change_result_json.is_some()
        || audit.command_result_json.is_some()
        || audit.tool_result_json.is_some()
        || audit.error.is_some()
        || audit.blocked_reason.is_some()
    {
        return Err(
            "manual approval execution audit is not an approved pre-execution receipt".to_string(),
        );
    }
    Ok(())
}

fn validate_manual_approval_execution_identity(
    pending: &AgentPendingActionRecord,
    audit: &AgentActionAuditRecord,
) -> Result<(), String> {
    if pending.action_id != audit.action_id
        || pending.run_id != audit.run_id
        || pending.conversation_id != audit.conversation_id
        || pending.assistant_message_id != audit.assistant_message_id
        || pending.action_type != audit.action_type
        || pending.tool_name != audit.tool_name
        || pending.action_json != audit.action_json
        || pending.created_at != audit.created_at
    {
        return Err(format!(
            "manual approval audit does not match the frozen pending action: actionId={}",
            pending.action_id
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPendingActionResultCommitOutcome {
    Committed { trace_changed: bool },
    Idempotent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentWakeApprovalWaitOutcome {
    Waiting(crate::AgentWakeRequestRecord),
    RunningAfterApproval(crate::AgentWakeRequestRecord),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequiredPendingToolProvenance {
    BuiltinCapabilityActivation,
    BuiltinCapabilityTool,
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

/// Extracts the Renderer/API action id from the framed durable storage identity. This remains
/// available even when the private action payload is malformed and cannot be decoded at startup.
fn renderer_action_id_from_pending_record(
    record: &AgentPendingActionRecord,
) -> Result<&str, String> {
    let prefix = format!("v2:{}:{}:", record.run_id.len(), record.run_id);
    let action_id = record
        .action_id
        .strip_prefix(&prefix)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "pending action durable identity is malformed".to_string())?;
    if crate::canonical_pending_action_id(&record.run_id, action_id) != record.action_id {
        return Err("pending action durable identity is malformed".to_string());
    }
    Ok(action_id)
}

fn resolve_pending_approval_notification_in_transaction(
    transaction: &rusqlite::Connection,
    record: &AgentPendingActionRecord,
    resolved_at: i64,
) -> Result<(), String> {
    let renderer_action_id = renderer_action_id_from_pending_record(record)?;
    notification_repository::resolve_notification_events_by_run_and_approval_action_id_in_transaction(
        transaction,
        &record.run_id,
        renderer_action_id,
        resolved_at,
    )
    .map_err(storage_error)?;
    Ok(())
}

fn is_valid_pending_successor(
    connection: &rusqlite::Connection,
    interrupted: &AgentPendingActionRecord,
    candidate: &AgentPendingActionRecord,
) -> Result<bool, String> {
    if !is_pending_successor_candidate(interrupted, candidate) {
        return Ok(false);
    }
    durable_trace_proves_action_precedes(interrupted, candidate, connection)
}

fn durable_trace_proves_action_precedes(
    predecessor: &AgentPendingActionRecord,
    successor: &AgentPendingActionRecord,
    connection: &rusqlite::Connection,
) -> Result<bool, String> {
    if predecessor.action_id == successor.action_id
        || predecessor.run_id != successor.run_id
        || predecessor.conversation_id != successor.conversation_id
        || predecessor.assistant_message_id != successor.assistant_message_id
    {
        return Ok(false);
    }
    let Ok(action) = serde_json::from_str::<AgentProposedAction>(&successor.action_json) else {
        return Ok(false);
    };
    let action_id = match &action {
        AgentProposedAction::ToolCall { call } => call.id.as_str(),
        AgentProposedAction::McpToolCall { approval } => approval.identity.call_id.as_str(),
        AgentProposedAction::BuiltinCapabilityActivation { approval } => approval.call_id.as_str(),
        AgentProposedAction::BuiltinMcpToolApproval { approval } => {
            approval.identity.call_id.as_str()
        }
        AgentProposedAction::BrowserRiskApproval { approval } => approval.call_id.as_str(),
        AgentProposedAction::FileChange { file_change } => file_change.id.as_str(),
        AgentProposedAction::Command { command } => command.id.as_str(),
        AgentProposedAction::SkillMaterialization { materialization } => {
            materialization.id.as_str()
        }
        AgentProposedAction::SkillScript { script } => script.id.as_str(),
        AgentProposedAction::OfficeOperation { office_operation } => office_operation.id.as_str(),
        AgentProposedAction::SkillInstallation { installation } => installation.id.as_str(),
    };
    if successor.tool_call_id.as_deref() != Some(action_id) {
        return Ok(false);
    }
    let Some(assistant_message_id) = successor.assistant_message_id.as_deref() else {
        return Ok(false);
    };
    let Some(trace) =
        conversation_trace_repository::get_trace_for_message(connection, assistant_message_id)
            .map_err(storage_error)?
    else {
        return Ok(false);
    };
    if trace.run_id != successor.run_id
        || trace.conversation_id != successor.conversation_id.as_deref().unwrap_or_default()
        || trace.assistant_message_id != assistant_message_id
    {
        return Ok(false);
    }
    let Some(parent_call_id) = predecessor.tool_call_id.as_deref() else {
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
        AgentProposedAction::FileChange { file_change } => {
            let digest = crate::file_change::proposal_digest(operation)
                .map_err(|_| "FileChange ToolCall arguments are invalid".to_string())?;
            if digest != file_change.execution.trace_args_digest {
                return Err(format!(
                    "启动对账发现 FileChange {action_id} 的冻结 ToolCall 参数与 action 不一致。"
                ));
            }
            file_change.summary.clone()
        }
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
        AgentProposedAction::McpToolCall { approval } => {
            if approval.call.args != *operation {
                return Err(format!(
                    "启动对账发现 MCP 操作 {action_id} 的冻结 ToolCall 参数不一致。"
                ));
            }
            approval.call.reason.clone()
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
pub(crate) fn manual_file_effect_has_authoritative_settlement(
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
        let is_mcp_action = matches!(action, AgentProposedAction::McpToolCall { .. });
        let is_file_change = matches!(action, AgentProposedAction::FileChange { .. });
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
        let expected_trace_approval_status = if is_mcp_action
            || (is_file_change
                && !matches!(audit.decision_source.as_deref(), Some("auto" | "run_grant")))
        {
            // Provider ToolCalls are immutable. MCP approval is represented by the terminal audit
            // and paired ToolResult rather than rewriting the frozen call from required.
            crate::AgentApprovalStatus::Required
        } else {
            crate::AgentApprovalStatus::Approved
        };
        if call_approval_status != expected_trace_approval_status {
            return Err("durable trace ToolCall approval state is inconsistent".to_string());
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
            approval_status: expected_trace_approval_status,
            reason,
        };
        let mut expected_result_item = crate::conversation_trace::projected_tool_result_trace_item(
            result_sequence,
            &expected_call,
            &audited_tool_result,
        );
        if is_file_change {
            let ConversationTurnTraceItem::ToolResult {
                approval_status, ..
            } = &mut expected_result_item
            else {
                return Err("file-effect projection did not produce a ToolResult".to_string());
            };
            *approval_status = match audit.decision.as_deref() {
                Some("approved") => crate::AgentApprovalStatus::Approved,
                Some("rejected" | "cancelled") => crate::AgentApprovalStatus::Rejected,
                _ => {
                    return Err(
                        "manual file-effect terminal audit has an invalid decision".to_string()
                    )
                }
            };
        }
        if is_mcp_action || is_file_change {
            // Archive metadata belongs to the validated trace rather than the typed terminal
            // audit. Current MCP and FileChange projections may attach that private archive
            // identity after the safe audit ToolResult is frozen, so exclude only this
            // trace-owned field from the semantic comparison.
            if let (
                ConversationTurnTraceItem::ToolResult {
                    archive: durable_archive,
                    ..
                },
                ConversationTurnTraceItem::ToolResult {
                    archive: expected_archive,
                    ..
                },
            ) = (
                &durable_trace.items[result_index],
                &mut expected_result_item,
            ) {
                *expected_archive = durable_archive.clone();
            }
        }
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
    resolve_pending_approval_notification_in_transaction(transaction, &record, updated_at)?;

    transaction
        .execute(
            "
            UPDATE agent_action_audit
            SET status = ?2,
                action_json = '{}',
                file_change_result_json = NULL,
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
        None,
    )
    .map_err(storage_error)?;
    if let Some(approval) = approval.as_ref() {
        let invocation = crate::mcp_tool_invocation_event(
            approval,
            crate::McpToolInvocationEventUpdate {
                state: match request.outcome {
                    McpStartupActionTerminalOutcome::PayloadUnavailable => {
                        crate::AgentMcpToolInvocationState::PayloadUnavailable
                    }
                    McpStartupActionTerminalOutcome::Expired => {
                        crate::AgentMcpToolInvocationState::Expired
                    }
                    McpStartupActionTerminalOutcome::PolicyDenied => {
                        crate::AgentMcpToolInvocationState::PolicyDenied
                    }
                    McpStartupActionTerminalOutcome::Rejected => {
                        crate::AgentMcpToolInvocationState::Rejected
                    }
                    McpStartupActionTerminalOutcome::Cancelled => {
                        crate::AgentMcpToolInvocationState::Cancelled
                    }
                    McpStartupActionTerminalOutcome::OutcomeUnknown => {
                        crate::AgentMcpToolInvocationState::OutcomeUnknown
                    }
                },
                dispatch_certainty: match request.outcome {
                    McpStartupActionTerminalOutcome::OutcomeUnknown => {
                        crate::AgentMcpDispatchCertainty::PossiblyDispatched
                    }
                    _ => crate::AgentMcpDispatchCertainty::DefinitelyNotDispatched,
                },
                outcome: Some(match request.outcome {
                    McpStartupActionTerminalOutcome::PayloadUnavailable => {
                        crate::AgentMcpToolInvocationOutcome::PayloadUnavailable
                    }
                    McpStartupActionTerminalOutcome::Expired => {
                        crate::AgentMcpToolInvocationOutcome::Expired
                    }
                    McpStartupActionTerminalOutcome::PolicyDenied => {
                        crate::AgentMcpToolInvocationOutcome::PolicyDenied
                    }
                    McpStartupActionTerminalOutcome::Rejected => {
                        crate::AgentMcpToolInvocationOutcome::Rejected
                    }
                    McpStartupActionTerminalOutcome::Cancelled => {
                        crate::AgentMcpToolInvocationOutcome::Cancelled
                    }
                    McpStartupActionTerminalOutcome::OutcomeUnknown => {
                        crate::AgentMcpToolInvocationOutcome::OutcomeUnknown
                    }
                }),
                is_error: request.outcome.invocation_is_error(),
                error_code: Some(request.outcome.error_code()),
                duration_ms: None,
                output_truncated: false,
                result_size: None,
                failure_stage: match request.outcome {
                    McpStartupActionTerminalOutcome::PayloadUnavailable
                    | McpStartupActionTerminalOutcome::Expired => {
                        Some(crate::AgentMcpInvocationFailureStage::ApprovalPayload)
                    }
                    McpStartupActionTerminalOutcome::PolicyDenied => {
                        Some(crate::AgentMcpInvocationFailureStage::Policy)
                    }
                    McpStartupActionTerminalOutcome::OutcomeUnknown => {
                        Some(crate::AgentMcpInvocationFailureStage::Shutdown)
                    }
                    McpStartupActionTerminalOutcome::Rejected
                    | McpStartupActionTerminalOutcome::Cancelled => None,
                },
            },
        )
        .map_err(|_| "MCP terminal lifecycle could not be projected safely".to_string())?;
        chat_repository::upsert_message_mcp_invocation_event(
            transaction,
            conversation_id,
            assistant_message_id,
            approval,
            &invocation,
            updated_at,
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
    super::trace_reconciliation::enqueue_reconciled_human_root_notification(
        transaction,
        &record.run_id,
        conversation_id,
        assistant_message_id,
        if run_status == "cancelled" {
            "task_cancelled"
        } else {
            "task_failed"
        },
        updated_at,
    )?;
    Ok(Some(record))
}

fn validate_builtin_capability_initial_audit(
    pending: &AgentPendingActionRecord,
    audit: &AgentActionAuditRecord,
) -> Result<(), String> {
    let action = serde_json::from_str::<AgentProposedAction>(&pending.action_json)
        .map_err(|_| "built-in capability pending action is invalid".to_string())?;
    let AgentProposedAction::BuiltinCapabilityActivation { approval } = action else {
        return Err("atomic built-in capability publication requires a typed activation".into());
    };
    let frozen_identity_matches = pending.action_id == audit.action_id
        && pending.run_id == audit.run_id
        && pending.conversation_id == audit.conversation_id
        && pending.assistant_message_id == audit.assistant_message_id
        && pending.action_type == "builtin_capability_activation"
        && audit.action_type == pending.action_type
        && pending.tool_name == "activate_capability"
        && audit.tool_name == pending.tool_name
        && pending.tool_call_id.as_deref() == Some(approval.call_id.as_str())
        && pending.run_id == approval.run_id
        && pending.action_json == audit.action_json
        && pending.created_at == audit.created_at;
    let initial_lifecycle_matches = pending.status == "pending"
        && pending.target_status.is_none()
        && audit.decision.is_none()
        && audit.status == "pending"
        && audit.file_change_result_json.is_none()
        && audit.command_result_json.is_none()
        && audit.tool_result_json.is_none()
        && audit.error.is_none()
        && audit.decided_at.is_none()
        && audit.completed_at.is_none()
        && audit.blocked_reason.is_none()
        && audit.decision_source.as_deref() == Some("manual_pending");
    if !frozen_identity_matches || !initial_lifecycle_matches {
        return Err(
            "built-in capability pending action and initial audit do not share one frozen identity"
                .to_string(),
        );
    }
    Ok(())
}

fn same_builtin_capability_initial_audit(
    existing: &AgentActionAuditRecord,
    candidate: &AgentActionAuditRecord,
) -> bool {
    existing.action_id == candidate.action_id
        && existing.run_id == candidate.run_id
        && existing.conversation_id == candidate.conversation_id
        && existing.assistant_message_id == candidate.assistant_message_id
        && existing.action_type == candidate.action_type
        && existing.tool_name == candidate.tool_name
        && existing.decision == candidate.decision
        && existing.status == candidate.status
        && existing.action_json == candidate.action_json
        && existing.file_change_result_json == candidate.file_change_result_json
        && existing.command_result_json == candidate.command_result_json
        && existing.tool_result_json == candidate.tool_result_json
        && existing.error == candidate.error
        && existing.created_at == candidate.created_at
        && existing.decided_at == candidate.decided_at
        && existing.completed_at == candidate.completed_at
        && existing.effective_permissions_json == candidate.effective_permissions_json
        && existing.path_scope == candidate.path_scope
        && existing.command_cwd_scope == candidate.command_cwd_scope
        && existing.blocked_reason == candidate.blocked_reason
        && existing.decision_source == candidate.decision_source
}

fn store_pending_action_or_conflict(
    connection: &rusqlite::Connection,
    record: &AgentPendingActionRecord,
) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
    let outcome = pending_action_repository::store_pending_action(connection, record)
        .map_err(storage_error)?;
    if let pending_action_repository::PendingActionStoreOutcome::Conflict {
        existing_run_id,
        existing_status,
    } = &outcome
    {
        return Err(format!(
            "待审批操作 actionId={} 已属于 runId={}（status={}）；拒绝覆盖冻结快照。",
            record.action_id, existing_run_id, existing_status
        ));
    }
    Ok(outcome)
}

fn ensure_exact_builtin_capability_initial_audit(
    connection: &rusqlite::Connection,
    audit: &AgentActionAuditRecord,
) -> Result<(), String> {
    if agent_action_audit_repository::insert_action_audit_record_if_absent(connection, audit)
        .map_err(storage_error)?
    {
        return Ok(());
    }
    let existing =
        agent_action_audit_repository::load_action_audit_record(connection, &audit.action_id)
            .map_err(storage_error)?
            .ok_or_else(|| {
                "built-in capability initial audit disappeared during publication".to_string()
            })?;
    if !same_builtin_capability_initial_audit(&existing, audit) {
        return Err(
            "built-in capability action id is already owned by a different audit identity"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_auto_file_change_initial_journal(
    pending: &AgentPendingActionRecord,
    audit: &AgentActionAuditRecord,
) -> Result<(), String> {
    let action = serde_json::from_str::<AgentProposedAction>(&pending.action_json)
        .map_err(|_| "automatic FileChange pending action JSON is invalid".to_string())?;
    let AgentProposedAction::FileChange { file_change } = action else {
        return Err("automatic FileChange journal requires the current action shape".to_string());
    };
    if pending.action_type != "file_change"
        || pending.tool_name != "apply_patch"
        || pending.tool_call_id.as_deref() != Some(file_change.id.as_str())
        || pending.status != "approved"
        || pending.target_status.is_some()
        || pending.action_id != audit.action_id
        || pending.run_id != audit.run_id
        || pending.conversation_id != audit.conversation_id
        || pending.assistant_message_id != audit.assistant_message_id
        || pending.action_type != audit.action_type
        || pending.tool_name != audit.tool_name
        || pending.action_json != audit.action_json
        || pending.created_at != audit.created_at
        || audit.decision.as_deref() != Some("approved")
        || audit.status != "approved"
        || audit.file_change_result_json.is_some()
        || audit.command_result_json.is_some()
        || audit.tool_result_json.is_some()
        || audit.error.is_some()
        || audit.decided_at != Some(audit.created_at)
        || audit.completed_at.is_some()
        || audit.blocked_reason.is_some()
        || !matches!(audit.decision_source.as_deref(), Some("auto" | "run_grant"))
        || audit.effective_permissions_json.is_none()
        || file_change.schema_version != crate::file_change::FILE_CHANGE_SCHEMA_VERSION
        || file_change.approval_status != crate::AgentApprovalStatus::Approved
        || file_change.execution.source_tool_name != "apply_patch"
        || file_change.execution.source_call_id != file_change.id
        || file_change.execution.run_id != pending.run_id
        || Some(file_change.execution.conversation_id.as_str())
            != pending.conversation_id.as_deref()
        || file_change.transaction_id != file_change.execution.transaction.id
        || file_change.execution.validate().is_err()
    {
        return Err("automatic FileChange pending/audit identity is inconsistent".to_string());
    }
    Ok(())
}

fn ensure_exact_auto_file_change_initial_audit(
    connection: &rusqlite::Connection,
    audit: &AgentActionAuditRecord,
) -> Result<(), String> {
    if agent_action_audit_repository::insert_action_audit_record_if_absent(connection, audit)
        .map_err(storage_error)?
    {
        return Ok(());
    }
    let existing =
        agent_action_audit_repository::load_action_audit_record(connection, &audit.action_id)
            .map_err(storage_error)?
            .ok_or_else(|| {
                "automatic FileChange audit disappeared during journal publication".to_string()
            })?;
    if !same_builtin_capability_initial_audit(&existing, audit) {
        return Err(
            "automatic FileChange action id is already owned by a different audit identity"
                .to_string(),
        );
    }
    Ok(())
}

fn is_current_file_change_action_identity(action_type: &str, tool_name: &str) -> bool {
    action_type == "file_change" && tool_name == "apply_patch"
}

#[derive(Clone, Copy)]
struct CurrentFileChangeActionIdentity<'a> {
    run_id: &'a str,
    conversation_id: Option<&'a str>,
    tool_call_id: Option<&'a str>,
    action_type: &'a str,
    tool_name: &'a str,
}

fn validate_file_change_action_json_pair(
    frozen_action_json: &str,
    expected_action_json: &str,
    committed_action_json: &str,
    identity: CurrentFileChangeActionIdentity<'_>,
) -> Result<(), String> {
    if expected_action_json.trim().is_empty()
        || committed_action_json.trim().is_empty()
        || expected_action_json == committed_action_json
        || frozen_action_json != expected_action_json
    {
        return Err("FileChange action JSON CAS input is invalid".to_string());
    }
    let prepared = serde_json::from_str::<AgentProposedAction>(expected_action_json)
        .map_err(|_| "FileChange prepared action JSON is invalid".to_string())?;
    let committed = serde_json::from_str::<AgentProposedAction>(committed_action_json)
        .map_err(|_| "FileChange committed action JSON is invalid".to_string())?;
    if !is_current_file_change_action_identity(identity.action_type, identity.tool_name) {
        return Err("FileChange action identity is invalid".to_string());
    }
    match (&prepared, &committed) {
        (
            AgentProposedAction::FileChange {
                file_change: prepared_change,
            },
            AgentProposedAction::FileChange {
                file_change: committed_change,
            },
        ) if identity.action_type == "file_change" && identity.tool_name == "apply_patch" => {
            use crate::file_change::FileChangeOperation;
            let operation_matches = matches!(
                (
                    prepared_change.operation,
                    prepared_change.execution.transaction.operation
                ),
                (
                    crate::AgentFileChangeOperation::Create,
                    FileChangeOperation::Create
                ) | (
                    crate::AgentFileChangeOperation::Update,
                    FileChangeOperation::Update
                ) | (
                    crate::AgentFileChangeOperation::Delete,
                    FileChangeOperation::Delete
                )
            );
            let strategy_matches = match prepared_change.operation {
                crate::AgentFileChangeOperation::Update
                    if prepared_change.execution.staged_transaction_id.is_some() =>
                {
                    matches!(
                        prepared_change.update_strategy,
                        Some(
                            crate::AgentFileChangeUpdateStrategy::Modify
                                | crate::AgentFileChangeUpdateStrategy::Rewrite
                        )
                    )
                }
                _ => prepared_change.update_strategy.is_none(),
            };
            let execution_transition_is_valid = committed_change
                .execution
                .is_commit_successor_of(&prepared_change.execution)
                || committed_change
                    .execution
                    .is_delete_finalization_successor_of(&prepared_change.execution);
            if !execution_transition_is_valid
                || prepared_change.schema_version != crate::file_change::FILE_CHANGE_SCHEMA_VERSION
                || prepared_change.id != committed_change.id
                || prepared_change.transaction_id != committed_change.transaction_id
                || prepared_change.operation != committed_change.operation
                || prepared_change.update_strategy != committed_change.update_strategy
                || prepared_change.file_path != committed_change.file_path
                || prepared_change.inline_diff != committed_change.inline_diff
                || prepared_change.base_revision != committed_change.base_revision
                || prepared_change.summary != committed_change.summary
                || prepared_change.additions != committed_change.additions
                || prepared_change.deletions != committed_change.deletions
                || prepared_change.line_count != committed_change.line_count
                || prepared_change.byte_count != committed_change.byte_count
                || prepared_change.approval_status != committed_change.approval_status
                || prepared_change.id != prepared_change.execution.source_call_id
                || prepared_change.execution.source_tool_name != "apply_patch"
                || prepared_change.transaction_id != prepared_change.execution.transaction.id
                || prepared_change.file_path != prepared_change.execution.transaction.file_path
                || prepared_change.base_revision.as_deref()
                    != prepared_change.execution.transaction.base.revision()
                || prepared_change.additions != prepared_change.execution.proposal.additions
                || prepared_change.deletions != prepared_change.execution.proposal.deletions
                || !operation_matches
                || !strategy_matches
                || prepared_change.execution.run_id != identity.run_id
                || Some(prepared_change.execution.conversation_id.as_str())
                    != identity.conversation_id
                || identity
                    .tool_call_id
                    .is_some_and(|call_id| call_id != prepared_change.id)
            {
                return Err(
                    "FileChange committed action is not the exact prepared successor".to_string(),
                );
            }
        }
        _ => return Err("FileChange action JSON has an invalid current shape".to_string()),
    }
    Ok(())
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
        invocation: Option<&crate::AgentMcpToolInvocationEvent>,
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
        if record.status != expected_status
            || !matches!(
                record.action_type.as_str(),
                "mcp_tool_call" | "builtin_mcp_tool_approval"
            )
        {
            return Ok(false);
        }
        let message_owner = match (
            record.conversation_id.as_deref(),
            record.assistant_message_id.as_deref(),
        ) {
            (Some(conversation_id), Some(assistant_message_id)) => {
                Some((conversation_id, assistant_message_id))
            }
            (None, None) => None,
            _ => {
                return Err("automatic MCP journal has an incomplete conversation owner".to_string())
            }
        };
        let action = serde_json::from_str::<AgentProposedAction>(&record.action_json)
            .map_err(|_| "automatic MCP journal contains an invalid frozen action".to_string())?;
        let action_type = record.action_type.clone();
        match action {
            AgentProposedAction::McpToolCall { approval } => {
                if action_type != "mcp_tool_call" {
                    return Err("automatic MCP journal action type is inconsistent".to_string());
                }
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
                    return Err(
                        "automatic MCP journal rejected a drifted typed identity".to_string()
                    );
                }
                if let Some(invocation) = invocation {
                    let lifecycle_matches_outcome = match outcome {
                        McpAutoActionJournalTerminalOutcome::Completed => {
                            invocation.state == crate::AgentMcpToolInvocationState::Completed
                        }
                        McpAutoActionJournalTerminalOutcome::Cancelled => {
                            invocation.state == crate::AgentMcpToolInvocationState::Cancelled
                        }
                        McpAutoActionJournalTerminalOutcome::OutcomeUnknown => {
                            invocation.state == crate::AgentMcpToolInvocationState::OutcomeUnknown
                        }
                        McpAutoActionJournalTerminalOutcome::Failed => matches!(
                            invocation.state,
                            crate::AgentMcpToolInvocationState::Failed
                                | crate::AgentMcpToolInvocationState::Expired
                                | crate::AgentMcpToolInvocationState::PayloadUnavailable
                                | crate::AgentMcpToolInvocationState::PolicyDenied
                        ),
                    };
                    if !lifecycle_matches_outcome {
                        return Err(
                            "automatic MCP journal lifecycle projection contradicts settlement"
                                .to_string(),
                        );
                    }
                    if let Some((conversation_id, assistant_message_id)) = message_owner {
                        chat_repository::upsert_message_mcp_invocation_event(
                            &transaction,
                            conversation_id,
                            assistant_message_id,
                            &approval,
                            invocation,
                            updated_at,
                        )
                        .map_err(storage_error)?;
                    }
                }
            }
            AgentProposedAction::BuiltinMcpToolApproval { approval } => {
                if action_type != "builtin_mcp_tool_approval" || invocation.is_some() {
                    return Err("automatic MCP journal action type is inconsistent".to_string());
                }
                crate::validate_builtin_mcp_tool_approval_shape(&approval)
                    .map_err(|_| "automatic built-in MCP journal shape is invalid".to_string())?;
                let identity = &approval.identity;
                let expected_storage_id = format!(
                    "v2:{}:{}:{}",
                    identity.run_id.len(),
                    identity.run_id,
                    identity.action_id
                );
                if approval.approval_status != crate::AgentApprovalStatus::Approved
                    || record.action_id != expected_storage_id
                    || record.run_id != identity.run_id
                    || record.tool_call_id.as_deref() != Some(identity.call_id.as_str())
                    || record.tool_name != identity.model_name
                {
                    return Err(
                        "automatic built-in MCP journal rejected a drifted typed identity"
                            .to_string(),
                    );
                }
            }
            _ => {
                return Err("automatic MCP journal action type is inconsistent".to_string());
            }
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
                  AND action_type = ?5
                ",
                rusqlite::params![
                    action_id,
                    expected_status,
                    terminal_status,
                    updated_at,
                    action_type
                ],
            )
            .map_err(storage_error)?;
        if affected != 1 {
            return Ok(false);
        }
        resolve_pending_approval_notification_in_transaction(&transaction, &record, updated_at)?;
        transaction
            .execute(
                "
                UPDATE agent_action_audit
                SET status = ?2,
                    action_json = '{}',
                    file_change_result_json = NULL,
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
        let terminalized =
            terminalize_mcp_action_in_transaction(&transaction, &request, updated_at)?;
        if let Some(record) = terminalized.as_ref() {
            file_change_run_grant_repository::revoke_nonterminal_run_grants(
                &transaction,
                &record.run_id,
                updated_at,
            )
            .map_err(storage_error)?;
        }
        let changed = terminalized.is_some();
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
        self.terminalize_generic_pending_action_as_failed(
            action_id,
            expected_status,
            updated_at,
            error_code,
            reason,
            None,
            None,
            None,
        )
    }

    /// Atomically expires one task-scoped built-in capability approval before activation.
    ///
    /// This path accepts only the typed activation action and its durable built-in Tool identity.
    /// It clears the public/private pending projections and terminates the owning run without ever
    /// creating a process-memory grant.
    pub fn expire_builtin_capability_agent_action(
        &self,
        action_id: &str,
        expected_status: &str,
        updated_at: i64,
    ) -> Result<bool, String> {
        if !matches!(expected_status, "pending" | "approved") {
            return Err("invalid built-in capability expiry status".to_string());
        }
        self.terminalize_generic_pending_action_as_failed(
            action_id,
            expected_status,
            updated_at,
            "builtin_capability.activation_expired",
            "The built-in capability activation approval expired before activation.",
            Some("builtin_capability_activation"),
            Some(RequiredPendingToolProvenance::BuiltinCapabilityActivation),
            None,
        )
    }

    /// Retires a sensitive built-in MCP Tool after process authority was lost.
    ///
    /// Raw arguments and the single-use grant are deliberately process-only. A restart therefore
    /// makes `pending`/`approved` definitely not dispatched, while `executing` is conservatively
    /// outcome-unknown. The unresolved original ToolCall receives exactly one bounded terminal
    /// ToolResult in the same append-only trace transaction; the Tool is never replayed.
    pub fn terminalize_builtin_mcp_tool_agent_action_on_startup(
        &self,
        action_id: &str,
        expected_status: &str,
        outcome: McpStartupActionTerminalOutcome,
        updated_at: i64,
    ) -> Result<bool, String> {
        if !valid_mcp_terminal_transition(expected_status, outcome)
            || !matches!(
                outcome,
                McpStartupActionTerminalOutcome::PayloadUnavailable
                    | McpStartupActionTerminalOutcome::Expired
                    | McpStartupActionTerminalOutcome::OutcomeUnknown
            )
        {
            return Err("invalid built-in MCP Tool startup terminal transition".to_string());
        }
        self.terminalize_generic_pending_action_as_failed(
            action_id,
            expected_status,
            updated_at,
            outcome.error_code(),
            outcome.safe_reason(),
            Some("builtin_mcp_tool_approval"),
            Some(RequiredPendingToolProvenance::BuiltinCapabilityTool),
            Some(outcome),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn terminalize_generic_pending_action_as_failed(
        &self,
        action_id: &str,
        expected_status: &str,
        updated_at: i64,
        error_code: &str,
        reason: &str,
        expected_action_type: Option<&str>,
        required_provenance: Option<RequiredPendingToolProvenance>,
        builtin_mcp_outcome: Option<McpStartupActionTerminalOutcome>,
    ) -> Result<bool, String> {
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
        if expected_action_type.is_some_and(|expected| record.action_type != expected) {
            return Err(
                "pending action terminalization rejected an unexpected action type".to_string(),
            );
        }
        let durable = load_durable_pending_trace_snapshot(
            &transaction,
            &record,
            expected_status != "executing",
        )?;
        let provenance_matches = match required_provenance {
            None => true,
            Some(RequiredPendingToolProvenance::BuiltinCapabilityActivation) => matches!(
                &durable.provenance,
                crate::AgentToolIdentity::RuntimeExtension {
                    extension_id,
                    tool_name,
                } if extension_id
                    == crate::builtin_capabilities::BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID
                    && tool_name == crate::builtin_capabilities::ACTIVATE_CAPABILITY_TOOL_NAME
            ),
            Some(RequiredPendingToolProvenance::BuiltinCapabilityTool) => matches!(
                &durable.provenance,
                crate::AgentToolIdentity::BuiltinCapability { .. }
            ),
        };
        if !provenance_matches {
            return Err(
                "pending action terminalization requires matching durable Tool provenance"
                    .to_string(),
            );
        }
        let conversation_id = record
            .conversation_id
            .as_deref()
            .ok_or_else(|| "malformed pending action requires a conversation owner".to_string())?;
        let assistant_message_id = record
            .assistant_message_id
            .as_deref()
            .ok_or_else(|| "malformed pending action requires an Assistant owner".to_string())?;
        let terminal = if let Some(outcome) = builtin_mcp_outcome {
            let action: crate::AgentProposedAction = serde_json::from_str(&record.action_json)
                .map_err(|_| "built-in MCP Tool action could not be decoded safely".to_string())?;
            let crate::AgentProposedAction::BuiltinMcpToolApproval { approval } = action else {
                return Err(
                    "built-in MCP Tool action type changed during terminalization".to_string(),
                );
            };
            let result = match outcome {
                McpStartupActionTerminalOutcome::PayloadUnavailable => {
                    crate::builtin_mcp_tool_payload_unavailable_result(&approval)
                }
                McpStartupActionTerminalOutcome::Expired => {
                    crate::builtin_mcp_tool_expired_result(&approval)
                }
                McpStartupActionTerminalOutcome::OutcomeUnknown => {
                    crate::builtin_mcp_tool_outcome_unknown_result(&approval)
                }
                McpStartupActionTerminalOutcome::PolicyDenied
                | McpStartupActionTerminalOutcome::Rejected
                | McpStartupActionTerminalOutcome::Cancelled => {
                    return Err("unsupported built-in MCP Tool terminal outcome".to_string())
                }
            };
            let result =
                crate::tools::builtin_capability_tool_result_persistence_projection(&result);
            crate::terminal_conversation_trace_from_snapshot_with_tool_result(
                durable.snapshot,
                &record.run_id,
                conversation_id,
                assistant_message_id,
                crate::ConversationTurnTraceTerminalStatus::Failed,
                reason,
                &result,
            )?
        } else {
            crate::terminal_conversation_trace_from_snapshot(
                durable.snapshot,
                &record.run_id,
                conversation_id,
                assistant_message_id,
                crate::ConversationTurnTraceTerminalStatus::Failed,
                reason,
            )?
        };

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
        // Unsupported/malformed startup rows must still retire without dispatch, but a non-current
        // durable identity is never reinterpreted as a Renderer approval id. There is no exact
        // notification target we can safely resolve for such a row.
        if expected_action_type.is_some() || renderer_action_id_from_pending_record(&record).is_ok()
        {
            resolve_pending_approval_notification_in_transaction(
                &transaction,
                &record,
                updated_at,
            )?;
        }
        transaction
            .execute(
                "
                UPDATE agent_action_audit
                SET status = 'failed',
                    action_json = '{}',
                    file_change_result_json = NULL,
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
            None,
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
        super::trace_reconciliation::enqueue_reconciled_human_root_notification(
            &transaction,
            &record.run_id,
            conversation_id,
            assistant_message_id,
            "task_failed",
            updated_at,
        )?;
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

    pub fn get_agent_action_audit(
        &self,
        action_id: &str,
    ) -> Result<Option<AgentActionAuditRecord>, String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::load_action_audit_record(&connection, action_id)
            .map_err(storage_error)
    }

    /// Returns current canonical executing FileChange audit records for startup reconciliation.
    /// Payload validation remains the recovery caller's responsibility.
    pub fn list_executing_file_change_action_audits(
        &self,
    ) -> Result<Vec<AgentActionAuditRecord>, String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::list_executing_file_change_action_audits(&connection)
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
        if !matches!(record.status.as_str(), "completed" | "failed" | "cancelled") {
            return Err(format!(
                "自动操作审计终态无效：{}（仅允许 completed、failed 或 cancelled）。",
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

    /// Persists the committed Direct FileChange credential for a manually approved action.
    ///
    /// Only the private `action_json` and its monotonic update timestamp change. The pending row
    /// must retain the exact frozen owner/Tool identity and remain `executing`.
    pub fn commit_pending_direct_file_change_action_json(
        &self,
        identity: &AgentPendingActionRecord,
        expected_action_json: &str,
        committed_action_json: &str,
        updated_at: i64,
    ) -> Result<AgentPendingActionJsonCommitOutcome, String> {
        if !is_current_file_change_action_identity(&identity.action_type, &identity.tool_name)
            || identity.tool_call_id.as_deref().is_none_or(str::is_empty)
            || identity.status != "executing"
            || identity.target_status.is_some()
            || updated_at < identity.updated_at
        {
            return Err("manual Direct FileChange pending identity is invalid".to_string());
        }
        validate_file_change_action_json_pair(
            &identity.action_json,
            expected_action_json,
            committed_action_json,
            CurrentFileChangeActionIdentity {
                run_id: &identity.run_id,
                conversation_id: identity.conversation_id.as_deref(),
                tool_call_id: identity.tool_call_id.as_deref(),
                action_type: &identity.action_type,
                tool_name: &identity.tool_name,
            },
        )?;
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = pending_action_repository::commit_executing_action_json(
            &transaction,
            identity,
            expected_action_json,
            committed_action_json,
            updated_at,
        )
        .map_err(storage_error)?;
        if matches!(
            outcome,
            AgentPendingActionJsonCommitOutcome::Updated
                | AgentPendingActionJsonCommitOutcome::AlreadyCommitted
        ) {
            let audit_outcome =
                agent_action_audit_repository::commit_pending_file_change_action_json_if_present(
                    &transaction,
                    identity,
                    expected_action_json,
                    committed_action_json,
                )
                .map_err(storage_error)?;
            match audit_outcome {
                agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::Updated
                | agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::AlreadyCommitted
                | agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::Absent => {}
                agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::ExpectedActionMismatch => {
                    return Err("manual Direct FileChange audit action JSON changed before commit"
                        .to_string());
                }
                agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::NotPreterminal {
                    status,
                } => {
                    return Err(format!(
                        "manual Direct FileChange audit is no longer preterminal: status={status}"
                    ));
                }
                agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::IdentityConflict {
                    status,
                } => {
                    return Err(format!(
                        "manual Direct FileChange audit identity conflict: status={status}"
                    ));
                }
            }
        }
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Publishes an actionable approval and its user notification as one visibility boundary.
    pub fn store_pending_agent_action_with_notification(
        &self,
        record: AgentPendingActionRecord,
        notification: &notification_repository::NewNotificationEventRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = store_pending_action_or_conflict(&transaction, &record)?;
        notification_repository::enqueue_notification_event_in_transaction(
            &transaction,
            notification,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Atomically publishes one built-in capability approval and its initial manual audit.
    ///
    /// A recoverable approval without its audit row is an externally observable split-brain:
    /// the approval UI can survive restart while the durable action journal omits the proposal.
    /// Keeping both inserts in one immediate transaction also makes an exact replay idempotent and
    /// rejects an action id already owned by a different audit identity without leaving an orphan
    /// pending row.
    pub fn store_builtin_capability_pending_action_with_audit(
        &self,
        pending: AgentPendingActionRecord,
        audit: AgentActionAuditRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_builtin_capability_initial_audit(&pending, &audit)?;
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = store_pending_action_or_conflict(&transaction, &pending)?;
        ensure_exact_builtin_capability_initial_audit(&transaction, &audit)?;
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Atomically persists the hidden auto-approval journal and its frozen audit before a
    /// FileChange may cross the filesystem effect boundary.
    ///
    /// The pending row owns the exact resumable ToolCall/checkpoint envelope. The audit owns the
    /// one allowed dispatch claim. Neither record is useful alone, so publishing both in one
    /// immediate transaction prevents a crash from leaving an executable but unrecoverable
    /// action.
    pub fn store_auto_file_change_pending_action_with_audit(
        &self,
        pending: AgentPendingActionRecord,
        audit: AgentActionAuditRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_auto_file_change_initial_journal(&pending, &audit)?;
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = store_pending_action_or_conflict(&transaction, &pending)?;
        ensure_exact_auto_file_change_initial_audit(&transaction, &audit)?;
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Atomically crosses the current automatic FileChange pre-dispatch boundary.
    ///
    /// Both the hidden Pending Action and its exact automatic approval audit must still be the
    /// byte-for-byte approved journal written by
    /// [`Self::store_auto_file_change_pending_action_with_audit`]. The filesystem committer may
    /// run only after this transaction returns `true`.
    #[allow(clippy::too_many_arguments)]
    pub fn claim_auto_file_change_pending_execution(
        &self,
        pending: &AgentPendingActionRecord,
        approved_audit: &AgentActionAuditRecord,
        expected_run_grant_ref: Option<&crate::file_change::FileChangeRunGrantRef>,
        expected_file_change: Option<&crate::AgentFileChangeProposal>,
        expected_run_context: Option<&crate::AgentRunContext>,
        executing_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<bool, String> {
        validate_auto_file_change_initial_journal(pending, approved_audit)?;
        let uses_run_grant = approved_audit.decision_source.as_deref() == Some("run_grant");
        if uses_run_grant
            != (expected_run_grant_ref.is_some()
                && expected_file_change.is_some()
                && expected_run_context.is_some())
            || (!uses_run_grant
                && (expected_file_change.is_some() || expected_run_context.is_some()))
        {
            return Err("automatic FileChange grant authority is inconsistent".to_string());
        }
        if let Some(grant_ref) = expected_run_grant_ref {
            grant_ref
                .validate()
                .map_err(|_| "automatic FileChange grant reference is invalid".to_string())?;
        }
        if updated_at < pending.updated_at || executing_agent_input_json.trim().is_empty() {
            return Err("automatic FileChange executing journal is invalid".to_string());
        }
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let Some(existing_pending) =
            pending_action_repository::load_pending_action(&transaction, &pending.action_id)
                .map_err(storage_error)?
        else {
            transaction.commit().map_err(storage_error)?;
            return Ok(false);
        };
        let pending_matches = existing_pending.action_id == pending.action_id
            && existing_pending.run_id == pending.run_id
            && existing_pending.conversation_id == pending.conversation_id
            && existing_pending.assistant_message_id == pending.assistant_message_id
            && existing_pending.action_type == pending.action_type
            && existing_pending.tool_name == pending.tool_name
            && existing_pending.tool_call_id == pending.tool_call_id
            && existing_pending.status == "approved"
            && existing_pending.target_status.is_none()
            && existing_pending.action_json == pending.action_json
            && existing_pending.agent_input_json == pending.agent_input_json
            && existing_pending.created_at == pending.created_at;
        let existing_audit = agent_action_audit_repository::load_action_audit_record(
            &transaction,
            &approved_audit.action_id,
        )
        .map_err(storage_error)?;
        let audit_matches = existing_audit.as_ref().is_some_and(|existing| {
            same_builtin_capability_initial_audit(existing, approved_audit)
        });
        if !pending_matches || !audit_matches {
            transaction.commit().map_err(storage_error)?;
            return Ok(false);
        }
        if let (Some(grant_ref), Some(file_change), Some(run_context)) = (
            expected_run_grant_ref,
            expected_file_change,
            expected_run_context,
        ) {
            let grant = file_change_run_grant_repository::get_active_run_grant(
                &transaction,
                &pending.run_id,
            )
            .map_err(storage_error)?;
            let Some(grant) = grant else {
                transaction.commit().map_err(storage_error)?;
                return Ok(false);
            };
            if !super::file_change_run_grants::file_change_run_grant_authorizes(
                &grant,
                Some(grant_ref),
                file_change,
                run_context,
            )
            .map_err(|error| error.to_string())?
            {
                transaction.commit().map_err(storage_error)?;
                return Ok(false);
            }
        }
        let pending_changed = pending_action_repository::transition_pending_action(
            &transaction,
            &pending.action_id,
            "approved",
            "executing",
            executing_agent_input_json,
            updated_at,
        )
        .map_err(storage_error)?;
        let audit_changed = transaction
            .execute(
                "
                UPDATE agent_action_audit
                SET status = 'executing'
                WHERE action_id = ?1
                  AND status = 'approved'
                  AND decision = 'approved'
                  AND decision_source = ?11
                  AND run_id = ?2
                  AND conversation_id IS ?3
                  AND assistant_message_id IS ?4
                  AND action_type = 'file_change'
                  AND tool_name = 'apply_patch'
                  AND action_json = ?5
                  AND created_at = ?6
                  AND decided_at IS ?7
                  AND effective_permissions_json IS ?8
                  AND path_scope IS ?9
                  AND command_cwd_scope IS ?10
                  AND file_change_result_json IS NULL
                  AND command_result_json IS NULL
                  AND tool_result_json IS NULL
                  AND error IS NULL
                  AND completed_at IS NULL
                  AND blocked_reason IS NULL
                ",
                rusqlite::params![
                    approved_audit.action_id,
                    approved_audit.run_id,
                    approved_audit.conversation_id,
                    approved_audit.assistant_message_id,
                    approved_audit.action_json,
                    approved_audit.created_at,
                    approved_audit.decided_at,
                    approved_audit.effective_permissions_json,
                    approved_audit.path_scope,
                    approved_audit.command_cwd_scope,
                    approved_audit.decision_source,
                ],
            )
            .map_err(storage_error)?;
        if pending_changed != 1 || audit_changed != 1 {
            return Err("automatic FileChange dispatch journal lost its exact CAS".to_string());
        }
        transaction.commit().map_err(storage_error)?;
        Ok(true)
    }

    pub fn store_builtin_capability_pending_action_with_audit_and_notification(
        &self,
        pending: AgentPendingActionRecord,
        audit: AgentActionAuditRecord,
        notification: &notification_repository::NewNotificationEventRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_builtin_capability_initial_audit(&pending, &audit)?;
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = store_pending_action_or_conflict(&transaction, &pending)?;
        ensure_exact_builtin_capability_initial_audit(&transaction, &audit)?;
        notification_repository::enqueue_notification_event_in_transaction(
            &transaction,
            notification,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Publishes a successor approval and terminalizes its predecessor as one durable fact.
    ///
    /// The predecessor's paired ToolResult and `target_status` were committed before model
    /// continuation. Once that continuation proposes another approval, the successor row becomes
    /// the recovery anchor for the Run. The insert and predecessor CAS must therefore commit
    /// together: exposing either half alone can leave an actionable orphan approval or lose the
    /// only continuation checkpoint.
    #[allow(clippy::too_many_arguments)]
    pub fn store_pending_agent_action_with_predecessor_settlement(
        &self,
        successor: AgentPendingActionRecord,
        predecessor_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        self.store_pending_agent_action_with_predecessor_settlement_internal(
            successor,
            predecessor_action_id,
            predecessor_expected_status,
            predecessor_terminal_status,
            predecessor_terminal_agent_input_json,
            updated_at,
            None,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn store_pending_agent_action_with_predecessor_settlement_and_notification(
        &self,
        successor: AgentPendingActionRecord,
        successor_notification: &notification_repository::NewNotificationEventRecord,
        predecessor_action_id: &str,
        predecessor_renderer_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        self.store_pending_agent_action_with_predecessor_settlement_internal(
            successor,
            predecessor_action_id,
            predecessor_expected_status,
            predecessor_terminal_status,
            predecessor_terminal_agent_input_json,
            updated_at,
            None,
            Some(successor_notification),
            Some(predecessor_renderer_action_id),
        )
    }

    /// Built-in capability variant of predecessor settlement. The successor pending row and its
    /// initial audit share the same transaction as the predecessor terminal CAS.
    #[allow(clippy::too_many_arguments)]
    pub fn store_builtin_capability_pending_action_with_predecessor_settlement_and_audit(
        &self,
        successor: AgentPendingActionRecord,
        successor_audit: AgentActionAuditRecord,
        predecessor_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_builtin_capability_initial_audit(&successor, &successor_audit)?;
        self.store_pending_agent_action_with_predecessor_settlement_internal(
            successor,
            predecessor_action_id,
            predecessor_expected_status,
            predecessor_terminal_status,
            predecessor_terminal_agent_input_json,
            updated_at,
            Some(successor_audit),
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn store_builtin_capability_pending_action_with_predecessor_settlement_audit_and_notification(
        &self,
        successor: AgentPendingActionRecord,
        successor_audit: AgentActionAuditRecord,
        successor_notification: &notification_repository::NewNotificationEventRecord,
        predecessor_action_id: &str,
        predecessor_renderer_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_builtin_capability_initial_audit(&successor, &successor_audit)?;
        self.store_pending_agent_action_with_predecessor_settlement_internal(
            successor,
            predecessor_action_id,
            predecessor_expected_status,
            predecessor_terminal_status,
            predecessor_terminal_agent_input_json,
            updated_at,
            Some(successor_audit),
            Some(successor_notification),
            Some(predecessor_renderer_action_id),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn store_pending_agent_action_with_predecessor_settlement_internal(
        &self,
        successor: AgentPendingActionRecord,
        predecessor_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
        successor_audit: Option<AgentActionAuditRecord>,
        successor_notification: Option<&notification_repository::NewNotificationEventRecord>,
        predecessor_renderer_action_id: Option<&str>,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        if successor.action_id == predecessor_action_id {
            return Err("successor approval must not replace its predecessor".to_string());
        }
        if successor.status != "pending" || successor.target_status.is_some() {
            return Err("successor approval must be a fresh pending action".to_string());
        }
        if !matches!(
            predecessor_terminal_status,
            "rejected" | "cancelled" | "completed" | "failed"
        ) {
            return Err("predecessor settlement requires a terminal target status".to_string());
        }

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let predecessor =
            pending_action_repository::load_pending_action(&transaction, predecessor_action_id)
                .map_err(storage_error)?
                .ok_or_else(|| "successor approval has no durable predecessor".to_string())?;
        if predecessor.run_id != successor.run_id
            || predecessor.conversation_id != successor.conversation_id
            || predecessor.assistant_message_id != successor.assistant_message_id
        {
            return Err("successor approval and predecessor ownership do not match".to_string());
        }
        if predecessor.target_status.as_deref() != Some(predecessor_terminal_status) {
            return Err(
                "predecessor terminal target was not durably committed before its successor"
                    .to_string(),
            );
        }
        let predecessor_already_terminal = predecessor.status == predecessor_terminal_status;
        if !predecessor_already_terminal && predecessor.status != predecessor_expected_status {
            return Err(format!(
                "predecessor status changed before successor publication: expected={predecessor_expected_status}, actual={}",
                predecessor.status
            ));
        }

        let outcome = store_pending_action_or_conflict(&transaction, &successor)?;

        if let Some(audit) = successor_audit.as_ref() {
            ensure_exact_builtin_capability_initial_audit(&transaction, audit)?;
        }

        if !predecessor_already_terminal {
            let affected = pending_action_repository::transition_pending_action(
                &transaction,
                predecessor_action_id,
                predecessor_expected_status,
                predecessor_terminal_status,
                predecessor_terminal_agent_input_json,
                updated_at,
            )
            .map_err(storage_error)?;
            if affected != 1 {
                return Err(format!(
                    "前置待审批操作终态迁移必须且只能更新一条记录，actionId={predecessor_action_id}，实际更新 {affected} 条。"
                ));
            }
        }
        if let Some(predecessor_renderer_action_id) = predecessor_renderer_action_id {
            notification_repository::resolve_notification_events_by_run_and_approval_action_id_in_transaction(
                &transaction,
                &successor.run_id,
                predecessor_renderer_action_id,
                updated_at,
            )
            .map_err(storage_error)?;
        }
        if let Some(notification) = successor_notification {
            notification_repository::enqueue_notification_event_in_transaction(
                &transaction,
                notification,
            )
            .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    pub fn list_pending_agent_actions(&self) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_pending_actions(&connection).map_err(storage_error)
    }

    /// Lists crash-interrupted rows for the Host's typed pre-reconciliation pass.
    ///
    /// Callers may inspect current-format private action receipts before the generic startup
    /// terminalizer runs. This is read-only and does not make an interrupted action replayable.
    pub fn list_interrupted_agent_actions_for_host_reconciliation(
        &self,
    ) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_interrupted_actions(&connection).map_err(storage_error)
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

    /// Returns whether a nonterminal action in the same durable Run must settle before the
    /// successor approval can resume model execution.
    ///
    /// The successor's frozen checkpoint selects candidate ToolResult ids, while the Conversation
    /// trace proves that a selected result is already durable before the successor ToolCall. This
    /// deliberately does not infer dependencies from creation time or sibling ToolCalls. Reading
    /// current SQLite status also avoids a false block when a terminal CAS committed but its
    /// caller observed an unknown response and process-local state remained stale.
    pub fn pending_agent_action_has_unsettled_predecessor(
        &self,
        successor_action_id: &str,
        frozen_result_call_ids: &[String],
    ) -> Result<bool, String> {
        if frozen_result_call_ids.is_empty() {
            return Ok(false);
        }
        let connection = self.state.connection()?;
        let Some(successor) =
            pending_action_repository::load_pending_action(&connection, successor_action_id)
                .map_err(storage_error)?
        else {
            // Preserve the approval path's existing durable status CAS as the authoritative
            // fail-closed boundary for a concurrently removed successor row. The predecessor
            // gate only adds dependency ordering; it must not replace missing-row arbitration.
            return Ok(false);
        };
        let unsettled =
            pending_action_repository::list_recoverable_actions_after_reconciliation(&connection)
                .map_err(storage_error)?;
        for predecessor in unsettled {
            if predecessor.tool_call_id.as_ref().is_some_and(|call_id| {
                frozen_result_call_ids
                    .iter()
                    .any(|frozen| frozen == call_id)
            }) && durable_trace_proves_action_precedes(&predecessor, &successor, &connection)?
            {
                return Ok(true);
            }
        }
        Ok(false)
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
            if matches!(record.action_type.as_str(), "mcp_tool_call" | "file_change")
                && record.status == "executing"
                && !manual_file_effect_has_authoritative_settlement(&transaction, record)?
            {
                return Err(format!(
                    "启动对账拒绝采用缺少权威审计与 ToolResult 证据的文件副作用目标终态：{}",
                    record.action_id
                ));
            }
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
            resolve_pending_approval_notification_in_transaction(&transaction, record, updated_at)?;
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
                resolve_pending_approval_notification_in_transaction(
                    &transaction,
                    candidate,
                    updated_at,
                )?;
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
                let notification_kind = match durable_run_status.as_deref() {
                    Some("completed") => "task_completed",
                    Some("failed") => "task_failed",
                    Some("cancelled") => "task_cancelled",
                    _ => unreachable!("terminal status was checked above"),
                };
                let conversation_id = record.conversation_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账发现终态 run {} 缺少 conversation owner。",
                        record.run_id
                    )
                })?;
                super::trace_reconciliation::enqueue_reconciled_human_root_notification(
                    &transaction,
                    &record.run_id,
                    conversation_id,
                    assistant_message_id,
                    notification_kind,
                    updated_at,
                )?;
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
                    None,
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
            super::trace_reconciliation::enqueue_reconciled_human_root_notification(
                &transaction,
                &record.run_id,
                record.conversation_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账无法通知 run {}：缺少 conversation owner。",
                        record.run_id
                    )
                })?,
                record.assistant_message_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账无法通知 run {}：缺少 Assistant owner。",
                        record.run_id
                    )
                })?,
                "task_failed",
                updated_at,
            )?;
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

    /// Resolves the approval notification in the same transaction that makes the approval stop
    /// being actionable. A crash cannot therefore leave a stale system notification behind.
    #[allow(clippy::too_many_arguments)]
    pub fn transition_pending_agent_action_and_resolve_notification(
        &self,
        action_id: &str,
        expected_status: &str,
        status: &str,
        agent_input_json: &str,
        run_id: &str,
        renderer_action_id: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let affected = pending_action_repository::transition_pending_action(
            &transaction,
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
        if status != "pending" {
            notification_repository::resolve_notification_events_by_run_and_approval_action_id_in_transaction(
                &transaction,
                run_id,
                renderer_action_id,
                updated_at,
            )
            .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }

    /// Atomically makes a cancelled approval non-actionable and revokes every nonterminal
    /// FileChange Run grant owned by that logical Run.
    ///
    /// Cancellation terminates the Run rather than continuing the model. Keeping the pending
    /// status CAS, notification settlement, and grant revocation in one immediate transaction
    /// prevents a terminal UI result from racing still-active remembered write authority.
    #[allow(clippy::too_many_arguments)]
    pub fn transition_cancelled_pending_agent_action_and_revoke_file_change_run_grants(
        &self,
        action_id: &str,
        expected_status: &str,
        agent_input_json: &str,
        run_id: &str,
        renderer_action_id: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let affected = pending_action_repository::transition_pending_action(
            &transaction,
            action_id,
            expected_status,
            "cancelled",
            agent_input_json,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作取消必须且只能更新一条记录，actionId={action_id}，实际更新 {affected} 条。"
            ));
        }
        notification_repository::resolve_notification_events_by_run_and_approval_action_id_in_transaction(
            &transaction,
            run_id,
            renderer_action_id,
            updated_at,
        )
        .map_err(storage_error)?;
        file_change_run_grant_repository::revoke_nonterminal_run_grants(
            &transaction,
            run_id,
            updated_at,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    /// Marks one exact child Wake as approval-paused only while its owning action is still
    /// pending in the same SQLite write transaction.
    ///
    /// A dispatcher observation can be older than a concurrent user decision. Serializing the
    /// pending-action recheck with the Wake CAS prevents that stale observation from moving an
    /// already resumed Wake back to `waiting_for_approval`.
    #[allow(clippy::too_many_arguments)]
    pub fn mark_agent_wake_waiting_for_pending_approval_at(
        &self,
        wake_id: &str,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        claim_token: &str,
        marked_at: i64,
    ) -> Result<AgentWakeApprovalWaitOutcome, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let running = agent_graph_repository::transition_agent_wake_in_connection(
            &transaction,
            wake_id,
            crate::AgentWakeStatus::Running,
            crate::AgentWakeStatus::Running,
            Some(claim_token),
            marked_at,
        )
        .map_err(|error| error.to_string())?;
        if running.run_id.as_deref() != Some(run_id)
            || running.assistant_message_id.as_deref() != Some(assistant_message_id)
        {
            return Err(
                "approval wait transition does not own the dispatched child Turn".to_string(),
            );
        }
        let pending_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*)
                 FROM agent_pending_actions
                 WHERE run_id = ?1
                   AND conversation_id = ?2
                   AND assistant_message_id = ?3
                   AND status = 'pending'
                   AND target_status IS NULL",
                rusqlite::params![run_id, conversation_id, assistant_message_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let outcome = match pending_count {
            0 => AgentWakeApprovalWaitOutcome::RunningAfterApproval(running),
            1 => {
                let waiting = agent_graph_repository::transition_agent_wake_in_connection(
                    &transaction,
                    wake_id,
                    crate::AgentWakeStatus::Running,
                    crate::AgentWakeStatus::WaitingForApproval,
                    Some(claim_token),
                    marked_at,
                )
                .map_err(|error| error.to_string())?;
                AgentWakeApprovalWaitOutcome::Waiting(waiting)
            }
            count => {
                return Err(format!(
                    "approval wait transition found {count} pending actions for one child Turn"
                ));
            }
        };
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Atomically accepts one Skill-script approval and, for a delegated Turn, resumes its exact
    /// Wake.
    ///
    /// The side effect may only be started after this transaction commits. Keeping the approved
    /// audit, the pending-action execution claim, and the child Wake in one CAS boundary prevents
    /// the durable `approved + waiting_for_approval` split-brain that otherwise has no executor.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_pending_skill_script_approval_execution(
        &self,
        action_id: &str,
        executing_agent_input_json: &str,
        approved_audit: &AgentActionAuditRecord,
        child_wake: Option<(&str, crate::AgentWakeStatus, &str)>,
        committed_at: i64,
    ) -> Result<(), String> {
        validate_manual_approval_execution_audit(action_id, approved_audit, committed_at)?;
        serde_json::from_str::<serde_json::Value>(executing_agent_input_json)
            .map_err(|_| "manual approval execution checkpoint is not valid JSON".to_string())?;

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let pending = pending_action_repository::load_pending_action(&transaction, action_id)
            .map_err(storage_error)?
            .ok_or_else(|| format!("manual approval has no pending action: {action_id}"))?;
        validate_manual_approval_execution_identity(&pending, approved_audit)?;
        if pending.action_type != "skill_script" || pending.tool_name != "skills_run_script" {
            return Err(
                "manual Skill-script approval does not own a Skill-script pending action"
                    .to_string(),
            );
        }
        if pending.status != "pending" || pending.target_status.is_some() {
            return Err(format!(
                "manual approval execution lost its pending CAS: actionId={action_id}, status={}",
                pending.status
            ));
        }
        if let Some(existing) =
            agent_action_audit_repository::load_action_audit_record(&transaction, action_id)
                .map_err(storage_error)?
        {
            if !agent_action_audit_repository::matches_manual_preterminal_action_audit(
                &existing,
                approved_audit,
            ) {
                return Err(format!(
                    "manual approval audit identity changed before execution: actionId={action_id}"
                ));
            }
        }

        let changed = pending_action_repository::transition_pending_action(
            &transaction,
            action_id,
            "pending",
            "executing",
            executing_agent_input_json,
            committed_at,
        )
        .map_err(storage_error)?;
        if changed != 1 {
            return Err(format!(
                "manual approval execution must claim exactly one pending action: actionId={action_id}, changed={changed}"
            ));
        }
        agent_action_audit_repository::upsert_action_audit_record(&transaction, approved_audit)
            .map_err(storage_error)?;

        if let Some((wake_id, expected_wake_status, claim_token)) = child_wake {
            if !matches!(
                expected_wake_status,
                crate::AgentWakeStatus::Running | crate::AgentWakeStatus::WaitingForApproval
            ) {
                return Err(
                    "manual approval child Wake must already own an active Turn".to_string()
                );
            }
            let resumed = agent_graph_repository::transition_agent_wake_in_connection(
                &transaction,
                wake_id,
                expected_wake_status,
                crate::AgentWakeStatus::Running,
                Some(claim_token),
                committed_at,
            )
            .map_err(|error| error.to_string())?;
            if resumed.run_id.as_deref() != Some(pending.run_id.as_str())
                || resumed.assistant_message_id.as_deref()
                    != pending.assistant_message_id.as_deref()
            {
                return Err(
                    "manual approval child Wake does not own the pending action Turn".to_string(),
                );
            }

            let conversation_id = pending.conversation_id.as_deref().ok_or_else(|| {
                "manual approval child Skill script has no conversation owner".to_string()
            })?;
            let assistant_message_id =
                pending.assistant_message_id.as_deref().ok_or_else(|| {
                    "manual approval child Skill script has no Assistant owner".to_string()
                })?;
            let tool_call_id = pending.tool_call_id.as_deref().ok_or_else(|| {
                "manual approval child Skill script has no ToolCall owner".to_string()
            })?;
            chat_repository::update_message_run_running_state(
                &transaction,
                conversation_id,
                assistant_message_id,
                &pending.run_id,
                tool_call_id,
                committed_at,
            )
            .map_err(storage_error)?;
            let usage_changed = transaction
                .execute(
                    "UPDATE agent_usage_records
                     SET status = 'running', error = NULL, completed_at = NULL
                     WHERE run_id = ?1
                       AND conversation_id = ?2
                       AND message_id = ?3
                       AND status = 'waiting_for_approval'",
                    rusqlite::params![&pending.run_id, conversation_id, assistant_message_id,],
                )
                .map_err(storage_error)?;
            if usage_changed != 1 {
                return Err(format!(
                    "manual approval child Skill script must resume exactly one waiting Usage row: actionId={action_id}, changed={usage_changed}"
                ));
            }
        }

        transaction.commit().map_err(storage_error)?;
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
        let pending_record =
            pending_action_repository::load_pending_action(&transaction, action_id)
                .map_err(storage_error)?
                .ok_or_else(|| {
                    "pre-Runtime continuation pending action no longer exists".to_string()
                })?;
        if pending_record.run_id != trace.run_id
            || pending_record.conversation_id.as_deref() != Some(conversation_id)
            || pending_record.assistant_message_id.as_deref() != Some(assistant_message_id)
        {
            return Err("pre-Runtime continuation pending identity changed".to_string());
        }
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
        resolve_pending_approval_notification_in_transaction(
            &transaction,
            &pending_record,
            completed_at,
        )?;
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
            None,
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
        super::trace_reconciliation::enqueue_reconciled_human_root_notification(
            &transaction,
            &trace.run_id,
            conversation_id,
            assistant_message_id,
            "task_failed",
            completed_at,
        )?;
        file_change_run_grant_repository::revoke_nonterminal_run_grants(
            &transaction,
            &trace.run_id,
            completed_at,
        )
        .map_err(storage_error)?;
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

        if terminal_audit.action_type == "file_change" {
            let result_json = terminal_audit
                .file_change_result_json
                .as_deref()
                .ok_or_else(|| "FileChange terminal audit lacks its typed result".to_string())?;
            let result = serde_json::from_str::<crate::AgentFileChangeResult>(result_json)
                .map_err(|_| "FileChange terminal audit result is invalid".to_string())?;
            let activate = matches!(
                result.status,
                crate::AgentFileChangeResultStatus::Applied
                    | crate::AgentFileChangeResultStatus::AlreadyApplied
            );
            let activation_result_digest =
                activate.then(|| crate::file_change::content_digest(result_json.as_bytes()));
            file_change_run_grant_repository::settle_pending_run_grant(
                &transaction,
                &pending.action_id,
                activate,
                activation_result_digest.as_deref(),
                committed_at,
            )
            .map_err(storage_error)?;
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
        let grant_intent = file_change_run_grant_repository::get_run_grant_for_pending_action(
            &transaction,
            &pending.action_id,
        )
        .map_err(storage_error)?;
        let expected_grant_active = terminal_audit
            .file_change_result_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<crate::AgentFileChangeResult>(json).ok())
            .is_some_and(|result| {
                matches!(
                    result.status,
                    crate::AgentFileChangeResultStatus::Applied
                        | crate::AgentFileChangeResultStatus::AlreadyApplied
                )
            });
        let grant_is_terminal = grant_intent.as_ref().is_none_or(|grant| {
            grant.status
                == if expected_grant_active {
                    crate::file_change::FileChangeRunGrantStatus::Active
                } else {
                    crate::file_change::FileChangeRunGrantStatus::Inactive
                }
        });
        let grant_is_pending = grant_intent.as_ref().is_none_or(|grant| {
            grant.status == crate::file_change::FileChangeRunGrantStatus::Pending
        });

        if target_is_committed && audit_is_terminal {
            if !grant_is_terminal {
                return Ok(settlement_diverged(
                    "fileChangeRunGrant",
                    "the terminal receipt committed without the exact grant lifecycle",
                ));
            }
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
            if !grant_is_pending {
                return Ok(settlement_diverged(
                    "fileChangeRunGrant",
                    "the grant lifecycle advanced without the terminal receipt",
                ));
            }
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

#[cfg(test)]
mod pending_action_identity_tests {
    use super::*;

    fn pending_record(action_id: &str, run_id: &str) -> AgentPendingActionRecord {
        AgentPendingActionRecord {
            action_id: action_id.to_string(),
            run_id: run_id.to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("assistant-1".to_string()),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_call_id: Some("call-1".to_string()),
            status: "pending".to_string(),
            target_status: None,
            action_json: "{}".to_string(),
            agent_input_json: "{}".to_string(),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn renderer_action_id_requires_exact_current_canonical_framing() {
        let run_id = "run:1";
        let canonical = crate::canonical_pending_action_id(run_id, "call:1");
        let current = pending_record(&canonical, run_id);
        assert_eq!(
            renderer_action_id_from_pending_record(&current).unwrap(),
            "call:1"
        );

        for malformed in [
            "call:1".to_string(),
            "v2:3:run:call:1".to_string(),
            "v2:5:run:2:".to_string(),
        ] {
            assert_eq!(
                renderer_action_id_from_pending_record(&pending_record(&malformed, run_id)),
                Err("pending action durable identity is malformed".to_string())
            );
        }
    }
}
