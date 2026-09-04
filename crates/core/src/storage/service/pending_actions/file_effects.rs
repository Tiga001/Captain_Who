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

