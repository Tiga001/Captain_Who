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
    TreeStopped(crate::AgentWakeRequestRecord),
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

