use super::*;

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

pub(super) fn is_valid_pending_successor(
    interrupted: &AgentPendingActionRecord,
    candidate: &AgentPendingActionRecord,
) -> bool {
    if !is_pending_successor_candidate(interrupted, candidate) {
        return false;
    }
    let Ok(action) = serde_json::from_str::<AgentProposedAction>(&candidate.action_json) else {
        return false;
    };
    let action_id = match &action {
        AgentProposedAction::ToolCall { call } => call.id.as_str(),
        AgentProposedAction::Diff { diff } => diff.id.as_str(),
        AgentProposedAction::FileWrite { file_write } => file_write.id.as_str(),
        AgentProposedAction::Command { command } => command.id.as_str(),
        AgentProposedAction::SkillMaterialization { materialization } => {
            materialization.id.as_str()
        }
        AgentProposedAction::SkillScript { script } => script.id.as_str(),
        AgentProposedAction::OfficeOperation { office_operation } => office_operation.id.as_str(),
    };
    if candidate.tool_call_id.as_deref() != Some(action_id) {
        return false;
    }
    let Ok(input) = serde_json::from_str::<AgentChatInput>(&candidate.agent_input_json) else {
        return false;
    };
    let Some(checkpoint) = input.resume_checkpoint.as_ref() else {
        return false;
    };
    if checkpoint.run_id != candidate.run_id || checkpoint.pending_tool_call_id != action_id {
        return false;
    }
    let Some(parent_call_id) = interrupted.tool_call_id.as_deref() else {
        return false;
    };
    let parent_result_sequence =
        checkpoint
            .conversation_trace_items
            .iter()
            .find_map(|item| match item {
                ConversationTurnTraceItem::ToolResult {
                    sequence, call_id, ..
                } if call_id == parent_call_id => Some(*sequence),
                _ => None,
            });
    let child_call_sequence =
        checkpoint
            .conversation_trace_items
            .iter()
            .find_map(|item| match item {
                ConversationTurnTraceItem::ToolCall {
                    sequence, call_id, ..
                } if call_id == action_id => Some(*sequence),
                _ => None,
            });
    matches!(
        (parent_result_sequence, child_call_sequence),
        (Some(parent), Some(child)) if parent < child
    )
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

/// Repairs the split-commit shape produced by older manual file-effect execution paths.
///
/// Those versions could commit a truthful terminal audit before the pending target and paired
/// trace. The audit is authoritative only after it matches the frozen pending action and the
/// suspended checkpoint. Recovery never re-executes the command; it only republishes the exact
/// durable ToolResult inside the caller's startup-reconciliation transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LegacyManualFileEffectRecovery {
    NotApplicable,
    Recovered(String),
    /// A write-ahead terminal target proves that effects may have occurred, but the remaining
    /// audit/trace evidence cannot prove the exact result. The lifecycle row may be retired, while
    /// `list_unsettled_file_effects` must keep the effect as a deletion blocker.
    Unverifiable,
}

fn recover_legacy_manual_file_effect_terminal_settlement(
    connection: &rusqlite::Connection,
    pending: &AgentPendingActionRecord,
    recovered_at: i64,
) -> Result<LegacyManualFileEffectRecovery, String> {
    if !matches!(
        (pending.action_type.as_str(), pending.tool_name.as_str()),
        ("command", "run_command")
            | ("skill_materialization", "skills_materialize_resource")
            | ("skill_script", "skills_run_script")
            | ("office_operation", "office_document")
            | ("office_operation", "office_spreadsheet")
            | ("office_operation", "office_presentation")
    ) {
        return Ok(LegacyManualFileEffectRecovery::NotApplicable);
    }

    let has_terminal_target = matches!(
        pending.target_status.as_deref(),
        Some("completed" | "failed" | "cancelled")
    );

    let Some(audit) =
        agent_action_audit_repository::load_action_audit_record(connection, &pending.action_id)
            .map_err(storage_error)?
    else {
        return Ok(if has_terminal_target {
            LegacyManualFileEffectRecovery::Unverifiable
        } else {
            LegacyManualFileEffectRecovery::NotApplicable
        });
    };
    if !matches!(audit.status.as_str(), "completed" | "failed" | "cancelled") {
        if matches!(
            audit.status.as_str(),
            "pending" | "approved" | "executing" | "cancellation_requested"
        ) {
            return Ok(if has_terminal_target {
                LegacyManualFileEffectRecovery::Unverifiable
            } else {
                LegacyManualFileEffectRecovery::NotApplicable
            });
        }
        if has_terminal_target {
            return Ok(LegacyManualFileEffectRecovery::Unverifiable);
        }
        return Err(format!(
            "启动对账发现人工命令 {} 的 audit 状态无效：{}",
            pending.action_id, audit.status
        ));
    }

    let validation = (|| {
        let completed_at = audit.completed_at.ok_or_else(|| {
            format!(
                "启动对账发现人工命令 {} 的 terminal audit 缺少 completed_at。",
                pending.action_id
            )
        })?;
        let tool_result = serde_json::from_str::<AgentToolResult>(
            audit.tool_result_json.as_deref().ok_or_else(|| {
                format!(
                    "启动对账发现人工命令 {} 的 terminal audit 缺少 ToolResult。",
                    pending.action_id
                )
            })?,
        )
        .map_err(|error| {
            format!(
                "启动对账无法解析人工命令 {} 的 terminal ToolResult：{error}",
                pending.action_id
            )
        })?;
        let trace = recovered_manual_file_effect_trace(pending, &tool_result)?;

        // Reuse the live settlement validators so startup recovery cannot accept a weaker identity
        // or terminal payload than a normal post-execution commit.
        validate_manual_file_effect_settlement_request(
            &audit,
            &pending.status,
            &audit.status,
            &trace,
            completed_at,
        )
        .map_err(|error| {
            format!(
                "启动对账拒绝恢复人工命令 {} 的 terminal audit：{error}",
                pending.action_id
            )
        })?;
        validate_manual_file_effect_settlement_identity(
            pending,
            &audit,
            &pending.status,
            &audit.status,
            &trace,
        )
        .map_err(|error| {
            format!(
                "启动对账发现人工命令 {} 的 terminal audit 身份冲突：{error}",
                pending.action_id
            )
        })?;
        Ok::<_, String>((completed_at, trace))
    })();
    let (completed_at, trace) = match validation {
        Ok(validated) => validated,
        Err(_) if has_terminal_target => {
            return Ok(LegacyManualFileEffectRecovery::Unverifiable);
        }
        Err(error) => return Err(error),
    };

    let affected = pending_action_repository::set_pending_action_target_status(
        connection,
        &pending.action_id,
        &pending.status,
        &audit.status,
        recovered_at,
    )
    .map_err(storage_error)?;
    if affected != 1 {
        return Err(format!(
            "启动对账无法修复人工命令 {} 的目标终态。",
            pending.action_id
        ));
    }
    let durable_trace = conversation_trace_repository::get_trace_for_message(
        connection,
        &trace.assistant_message_id,
    )
    .map_err(storage_error)?;
    match manual_settlement_trace_state(durable_trace.as_ref(), &trace) {
        ManualSettlementTraceState::Absent | ManualSettlementTraceState::BeforeBoundary => {
            conversation_trace_repository::commit_trace_in_connection(
                connection,
                &trace,
                pending.created_at,
                recovered_at.max(completed_at),
            )
            .map_err(storage_error)?;
        }
        ManualSettlementTraceState::AtBoundary | ManualSettlementTraceState::Advanced => {}
        ManualSettlementTraceState::Diverged(reason) => {
            if has_terminal_target {
                return Ok(LegacyManualFileEffectRecovery::Unverifiable);
            }
            return Err(format!(
                "启动对账发现人工命令 {} 的 durable trace 冲突：{reason}",
                pending.action_id
            ));
        }
    }

    Ok(LegacyManualFileEffectRecovery::Recovered(audit.status))
}

fn recovered_manual_file_effect_trace(
    pending: &AgentPendingActionRecord,
    tool_result: &AgentToolResult,
) -> Result<ConversationTurnTrace, String> {
    let input =
        serde_json::from_str::<AgentChatInput>(&pending.agent_input_json).map_err(|error| {
            format!(
                "启动对账无法解析人工命令 {} 的冻结续跑输入：{error}",
                pending.action_id
            )
        })?;
    if input.tool_continuation.is_some() {
        return Err(format!(
            "启动对账发现人工命令 {} 的冻结输入已包含 ToolResult continuation。",
            pending.action_id
        ));
    }
    let checkpoint = input.resume_checkpoint.as_ref().ok_or_else(|| {
        format!(
            "启动对账无法恢复人工命令 {}：冻结输入缺少运行检查点。",
            pending.action_id
        )
    })?;
    let call_id = pending.tool_call_id.as_deref().ok_or_else(|| {
        format!(
            "启动对账无法恢复人工命令 {}：pending action 缺少 tool call id。",
            pending.action_id
        )
    })?;
    let action =
        serde_json::from_str::<AgentProposedAction>(&pending.action_json).map_err(|error| {
            format!(
                "启动对账无法解析人工文件副作用 {} 的冻结 action：{error}",
                pending.action_id
            )
        })?;
    let (_, expected_tool, expected_call_id, _) = manual_file_effect_identity(&action)?;
    if checkpoint.run_id != pending.run_id
        || checkpoint.pending_tool_call_id != call_id
        || tool_result.call_id != call_id
        || tool_result.tool != expected_tool
        || expected_call_id != call_id
        || pending.tool_name != expected_tool
    {
        return Err(format!(
            "启动对账发现人工命令 {} 的 checkpoint、pending action 与 ToolResult 身份不一致。",
            pending.action_id
        ));
    }
    if checkpoint.conversation_trace_items.iter().any(
        |item| matches!(item, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id),
    ) {
        return Err(format!(
            "启动对账发现人工命令 {} 的冻结检查点已经包含待恢复 ToolResult。",
            pending.action_id
        ));
    }

    let matching_calls = checkpoint
        .conversation_trace_items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id: candidate,
                tool,
                operation,
                ..
            } if candidate == call_id => Some((tool, operation)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if matching_calls.len() != 1 {
        return Err(format!(
            "启动对账无法恢复人工命令 {}：冻结检查点必须包含唯一的原始 ToolCall。",
            pending.action_id
        ));
    }
    let (tool, operation) = matching_calls[0];
    let reason =
        validate_frozen_manual_file_effect_tool_call(&pending.action_id, &action, tool, operation)?;

    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: expected_tool,
        args: operation.clone(),
        approval_status: crate::AgentApprovalStatus::Approved,
        reason,
    };
    let conversation_id = pending.conversation_id.as_deref().ok_or_else(|| {
        format!(
            "启动对账无法恢复人工命令 {}：缺少 conversation id。",
            pending.action_id
        )
    })?;
    let assistant_message_id = pending.assistant_message_id.as_deref().ok_or_else(|| {
        format!(
            "启动对账无法恢复人工命令 {}：缺少 assistant message id。",
            pending.action_id
        )
    })?;
    let snapshot =
        crate::conversation_trace_snapshot_from_checkpoint_and_continuation_with_history_ref(
            checkpoint,
            &call,
            tool_result,
            Some(assistant_message_id),
        );
    Ok(snapshot.in_progress_trace(&pending.run_id, conversation_id, assistant_message_id))
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

impl StorageService {
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
            if !is_authoritatively_settled {
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

    pub fn reconcile_interrupted_pending_agent_actions(
        &self,
        updated_at: i64,
    ) -> Result<Vec<AgentPendingActionRecord>, String> {
        const INTERRUPTION_REASON: &str =
            "The application exited after approval; process outcome is unknown and was not replayed.";
        const RECOVERED_FILE_EFFECT_INTERRUPTION_REASON: &str =
            "The approved file-producing action result was recovered, but the application exited before the agent continuation completed.";
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
            let recovered_file_effect = recover_legacy_manual_file_effect_terminal_settlement(
                &transaction,
                record,
                updated_at,
            )?;
            let recovered_file_effect_status = match &recovered_file_effect {
                LegacyManualFileEffectRecovery::Recovered(status) => Some(status.as_str()),
                LegacyManualFileEffectRecovery::NotApplicable
                | LegacyManualFileEffectRecovery::Unverifiable => None,
            };
            let reconciled_status = match (
                record.target_status.as_deref(),
                recovered_file_effect_status,
            ) {
                (_, Some(status)) => status,
                (Some(status @ ("completed" | "failed" | "rejected" | "cancelled")), None) => {
                    status
                }
                (None, None) => {
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
                            "启动对账无法为旧待审批操作 {} 写入 failed 目标终态。",
                            record.action_id
                        ));
                    }
                    "failed"
                }
                (Some(status), None) => {
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
            let valid_successors = successor_candidates
                .iter()
                .copied()
                .filter(|candidate| is_valid_pending_successor(record, candidate))
                .collect::<Vec<_>>();
            if valid_successors.len() > 1 {
                return Err(format!(
                    "启动对账发现 action {} 存在多个合法待审批后继。",
                    record.action_id
                ));
            }
            for candidate in successor_candidates
                .iter()
                .copied()
                .filter(|candidate| !is_valid_pending_successor(record, candidate))
            {
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
            if let (Some(conversation_id), Some(message_id)) = (
                record.conversation_id.as_deref(),
                record.assistant_message_id.as_deref(),
            ) {
                chat_repository::update_message_run_terminal_state(
                    &transaction,
                    conversation_id,
                    message_id,
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
                    rusqlite::params![
                        record.run_id,
                        if matches!(
                            recovered_file_effect,
                            LegacyManualFileEffectRecovery::Recovered(_)
                        ) {
                            RECOVERED_FILE_EFFECT_INTERRUPTION_REASON
                        } else {
                            INTERRUPTION_REASON
                        },
                        updated_at
                    ],
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

    /// Atomically settles a manually approved file-producing action across every durable
    /// representation.
    ///
    /// The action audit, pending-action target, and paired ToolResult trace form one recovery
    /// boundary. Retrying the exact same bundle is idempotent; a different identity or terminal
    /// result fails closed without changing any of the three records.
    pub fn commit_pending_agent_action_audited_result_trace(
        &self,
        terminal_audit: &AgentActionAuditRecord,
        expected_pending_status: &str,
        target_status: &str,
        trace: &ConversationTurnTrace,
        committed_at: i64,
    ) -> Result<AgentPendingActionResultCommitOutcome, String> {
        self.commit_pending_agent_action_audited_result_trace_with_model_context(
            terminal_audit,
            expected_pending_status,
            target_status,
            trace,
            &[],
            committed_at,
        )
    }

    /// Same settlement boundary as [`Self::commit_pending_agent_action_audited_result_trace`],
    /// additionally committing the exact bounded model projection in the same transaction.
    ///
    /// The compatibility wrapper above remains for callers that have no model projection (for
    /// example, legacy data tests). New runtime settlement paths must use this method so a crash
    /// cannot leave an approved ToolResult visible only through the lossy trace fallback.
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

fn validate_manual_file_effect_settlement_request(
    audit: &AgentActionAuditRecord,
    expected_pending_status: &str,
    target_status: &str,
    trace: &ConversationTurnTrace,
    committed_at: i64,
) -> Result<(), String> {
    if !matches!(target_status, "completed" | "failed" | "cancelled")
        || audit.status != target_status
    {
        return Err(format!(
            "manual file-effect terminal status mismatch: audit={}, target={target_status}",
            audit.status
        ));
    }
    if audit.decision.as_deref() != Some("approved")
        || audit.decision_source.as_deref() != Some("manual")
        || audit.patch_result_json.is_some()
    {
        return Err(
            "manual file-effect settlement contains an invalid audit lifecycle".to_string(),
        );
    }
    let action = serde_json::from_str::<AgentProposedAction>(&audit.action_json)
        .map_err(|error| format!("frozen file-effect action is invalid: {error}"))?;
    let (expected_action_type, expected_tool, expected_call_id, is_command) =
        manual_file_effect_identity(&action)?;
    let pending_status_is_valid = expected_pending_status == "approved"
        || (expected_action_type == "skill_materialization"
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
    let command_result = match (is_command, audit.command_result_json.as_deref()) {
        (true, Some(command_result_json)) => Some(
            serde_json::from_str::<AgentCommandExecutionResult>(command_result_json)
                .map_err(|error| format!("manual command result is invalid: {error}"))?,
        ),
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
    let tool_result_json = audit
        .tool_result_json
        .as_deref()
        .ok_or_else(|| "manual file-effect terminal audit lacks tool_result_json".to_string())?;
    let tool_result = serde_json::from_str::<AgentToolResult>(tool_result_json)
        .map_err(|error| format!("manual file-effect ToolResult is invalid: {error}"))?;
    if let Some(command_result) = command_result.as_ref() {
        validate_manual_command_result_projection(command_result, &tool_result, target_status)?;
    }
    if tool_result.tool != expected_tool
        || tool_result.call_id != expected_call_id
        || (target_status == "completed") != tool_result.ok
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
    if call_id != &tool_result.call_id || tool != &expected_tool || *success != tool_result.ok {
        return Err(
            "manual file-effect trace result identity differs from terminal audit".to_string(),
        );
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
        || wrapper.get("execution") != Some(execution)
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
        _ => Err(
            "manual audited settlement only supports command, Office and Skill file effects"
                .to_string(),
        ),
    }
}
