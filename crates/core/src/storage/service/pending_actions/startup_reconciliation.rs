impl StorageService {
    /// Adopts the exact terminal `outcome_unknown` ToolResult when the MCP dispatch journal
    /// lagged behind an already-completed conversation trace.
    ///
    /// This is deliberately narrower than normal startup terminalization. It never manufactures
    /// a result, changes the terminal Assistant/run, or accepts an arbitrary failed ToolResult.
    /// The pending row, frozen action, audit identity, MCP provenance, paired Trace, and complete
    /// model-context projection must all agree before the stale journal is scrubbed.
    pub fn adopt_terminal_mcp_agent_action_on_startup(
        &self,
        action_id: &str,
        expected_status: &str,
        updated_at: i64,
    ) -> Result<bool, String> {
        if expected_status != "executing" {
            return Ok(false);
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
        if record.status != expected_status || record.target_status.is_some() {
            return Ok(false);
        }
        if record.action_type != "mcp_tool_call" {
            return Err("terminal MCP journal adoption rejected a non-MCP action".to_string());
        }

        let conversation_id = record
            .conversation_id
            .as_deref()
            .ok_or_else(|| "terminal MCP journal requires a conversation owner".to_string())?;
        let assistant_message_id = record
            .assistant_message_id
            .as_deref()
            .ok_or_else(|| "terminal MCP journal requires an Assistant owner".to_string())?;
        let Some(trace) = conversation_trace_repository::get_trace_for_message(
            &transaction,
            assistant_message_id,
        )
        .map_err(storage_error)?
        else {
            return Ok(false);
        };
        trace
            .validate()
            .map_err(|_| "terminal MCP journal ConversationTurnTrace is invalid".to_string())?;
        if trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::Completed
            || trace.terminal_error.is_some()
        {
            return Ok(false);
        }
        if trace.run_id != record.run_id
            || trace.conversation_id != conversation_id
            || trace.assistant_message_id != assistant_message_id
        {
            return Err("terminal MCP journal trace owner identity is inconsistent".to_string());
        }

        let model_context_items = conversation_model_context_repository::get_log_for_message(
            &transaction,
            assistant_message_id,
        )
        .map_err(storage_error)?
        .map(|log| log.items)
        .ok_or_else(|| "terminal MCP journal requires a durable model-context log".to_string())?;
        trace
            .validate_complete_model_context(&model_context_items)
            .map_err(|_| "terminal MCP journal model-context log is invalid".to_string())?;

        let row_call_id = record
            .tool_call_id
            .as_deref()
            .ok_or_else(|| "terminal MCP journal requires a ToolCall identity".to_string())?;
        let matching_calls = trace
            .items
            .iter()
            .filter(|item| {
                matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == row_call_id)
            })
            .collect::<Vec<_>>();
        let matching_results = trace
            .items
            .iter()
            .filter(|item| {
                matches!(item, ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == row_call_id)
            })
            .collect::<Vec<_>>();
        let [ConversationTurnTraceItem::ToolCall {
            sequence: call_sequence,
            call_id,
            tool,
            provenance,
            operation,
            approval_status: call_approval_status,
            ..
        }] = matching_calls.as_slice()
        else {
            return Err("terminal MCP journal requires exactly one matching ToolCall".to_string());
        };
        let [ConversationTurnTraceItem::ToolResult {
            sequence: result_sequence,
            tool: result_tool,
            status: result_status,
            success,
            observation,
            approval_status: result_approval_status,
            error,
            truncated,
            archive,
            ..
        }] = matching_results.as_slice()
        else {
            return Err(
                "terminal MCP journal requires exactly one matching ToolResult".to_string(),
            );
        };
        let crate::AgentToolIdentity::Mcp {
            provenance: mcp_provenance,
        } = provenance
        else {
            return Err("terminal MCP journal requires durable MCP Tool provenance".to_string());
        };
        if tool != &record.tool_name
            || result_tool != tool
            || result_sequence <= call_sequence
            || *call_approval_status != crate::AgentApprovalStatus::Approved
            || result_approval_status != call_approval_status
            || *result_status != crate::ConversationTraceToolResultStatus::Failed
            || *success
            || *truncated
            || archive != &crate::ConversationHistoryArchiveTraceMetadata::default()
        {
            return Err(
                "terminal MCP journal ToolCall/ToolResult pair is inconsistent".to_string(),
            );
        }

        let durable_result = AgentToolResult {
            call_id: call_id.clone(),
            tool: tool.clone(),
            ok: *success,
            result: Some(observation.clone()),
            error: error.clone(),
            exact_archive_file: None,
        };
        validate_durable_mcp_tool_result(&durable_result, "failed")?;
        let observation = observation.as_object().ok_or_else(|| {
            "terminal MCP outcome-unknown observation must be an object".to_string()
        })?;
        if observation
            .get("status")
            .and_then(serde_json::Value::as_str)
            != Some("outcome_unknown")
            || observation
                .get("outcome")
                .and_then(serde_json::Value::as_str)
                != Some("outcome_unknown")
            || observation.get("code").and_then(serde_json::Value::as_str)
                != Some("mcp.tool_outcome_unknown")
            || observation
                .get("dispatchCertainty")
                .and_then(serde_json::Value::as_str)
                != Some("possibly_dispatched")
            || observation
                .get("retryable")
                .and_then(serde_json::Value::as_bool)
                != Some(false)
        {
            return Err(
                "terminal MCP journal only adopts the canonical outcome-unknown result".to_string(),
            );
        }

        let call_model_items = model_context_items
            .iter()
            .filter(|item| item.sequence == *call_sequence)
            .collect::<Vec<_>>();
        let result_model_items = model_context_items
            .iter()
            .filter(|item| item.sequence == *result_sequence)
            .collect::<Vec<_>>();
        let [call_model_item] = call_model_items.as_slice() else {
            return Err("terminal MCP journal requires the exact ToolCall model item".to_string());
        };
        let [result_model_item] = result_model_items.as_slice() else {
            return Err(
                "terminal MCP journal requires the exact ToolResult model item".to_string(),
            );
        };
        if call_model_item.ordinal != 0
            || call_model_item.tool_calls.len() != 1
            || call_model_item.tool_calls[0].args != *operation
            || result_model_item.ordinal != 0
            || result_model_item.content
                != crate::conversation_trace::render_tool_observation(&durable_result)
        {
            return Err("terminal MCP journal exact model projection is inconsistent".to_string());
        }

        let durable = DurablePendingTraceSnapshot {
            snapshot: crate::ConversationTraceSnapshot {
                items: trace.items.clone(),
                model_context_items: model_context_items.clone(),
                next_sequence: trace
                    .items
                    .last()
                    .map(ConversationTurnTraceItem::sequence)
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| {
                        "terminal MCP journal trace sequence is exhausted".to_string()
                    })?,
                truncated: trace.truncated,
            },
            call: AgentToolCall {
                id: call_id.clone(),
                tool: tool.clone(),
                args: operation.clone(),
                approval_status: *call_approval_status,
                reason: operation
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            },
            provenance: crate::AgentToolIdentity::Mcp {
                provenance: mcp_provenance.clone(),
            },
        };
        let approval =
            validated_mcp_approval_for_durable_call(&record, &durable).ok_or_else(|| {
                "terminal MCP journal frozen action identity is inconsistent".to_string()
            })?;
        if approval.call.approval_status != crate::AgentApprovalStatus::Approved {
            return Err("terminal MCP journal frozen call is inconsistent".to_string());
        }
        if approval.call.args != *operation
            || !operation.as_object().is_some_and(serde_json::Map::is_empty)
        {
            return Err(
                "terminal MCP journal durable call is not the exact safe MCP projection"
                    .to_string(),
            );
        }

        let owner = transaction
            .query_row(
                "SELECT role, status, agent_run_json
                 FROM messages
                 WHERE conversation_id = ?1 AND id = ?2",
                rusqlite::params![conversation_id, assistant_message_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| "terminal MCP journal requires its owning message".to_string())?;
        let (owner_role, owner_status, owner_run_json) = owner;
        if owner_role != "assistant" || owner_status.as_deref() != Some("sent") {
            return Err("terminal MCP journal owning message is not completed".to_string());
        }
        let owner_run = owner_run_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| "terminal MCP journal requires a current owning run".to_string())?;
        if !chat_repository::current_agent_run_projection_is_safe(&owner_run, &record.run_id)
            || owner_run.get("runId").and_then(serde_json::Value::as_str)
                != Some(record.run_id.as_str())
            || owner_run.get("status").and_then(serde_json::Value::as_str) != Some("completed")
            || owner_run
                .get("completedAt")
                .and_then(serde_json::Value::as_i64)
                .is_none()
            || owner_run.contains_key("error")
        {
            return Err("terminal MCP journal owning run is not completed".to_string());
        }
        let owner_state = owner_run
            .get("state")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| "terminal MCP journal requires a completed run state".to_string())?;
        if owner_state
            .get("status")
            .and_then(serde_json::Value::as_str)
            != Some("completed")
            || !owner_state
                .get("activeRunId")
                .is_some_and(serde_json::Value::is_null)
            || !owner_state
                .get("lastError")
                .is_some_and(serde_json::Value::is_null)
        {
            return Err("terminal MCP journal owning run state is not completed".to_string());
        }
        let matching_invocations = owner_run
            .get("mcpInvocations")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "terminal MCP journal requires an MCP lifecycle".to_string())?
            .iter()
            .filter(|invocation| {
                invocation
                    .get("actionId")
                    .and_then(serde_json::Value::as_str)
                    == Some(approval.identity.action_id.as_str())
            })
            .collect::<Vec<_>>();
        let [invocation] = matching_invocations.as_slice() else {
            return Err(
                "terminal MCP journal requires exactly one matching MCP lifecycle".to_string(),
            );
        };
        let expected_scope = serde_json::to_value(&approval.identity.provenance.scope)
            .map_err(|_| "terminal MCP journal scope is invalid".to_string())?;
        if invocation
            .get("invocationId")
            .and_then(serde_json::Value::as_str)
            != Some(approval.identity.invocation_id.as_str())
            || invocation.get("callId").and_then(serde_json::Value::as_str)
                != Some(approval.identity.call_id.as_str())
            || invocation
                .get("serverId")
                .and_then(serde_json::Value::as_str)
                != Some(approval.identity.provenance.server_id.as_str())
            || invocation
                .get("serverDisplayName")
                .and_then(serde_json::Value::as_str)
                != Some(approval.summary.server_display_name.as_str())
            || invocation
                .get("rawToolName")
                .and_then(serde_json::Value::as_str)
                != Some(approval.identity.provenance.raw_tool_name.as_str())
            || invocation
                .get("modelToolName")
                .and_then(serde_json::Value::as_str)
                != Some(approval.identity.provenance.model_tool_name.as_str())
            || invocation
                .get("scope")
                .is_some_and(|scope| scope != &expected_scope)
            || invocation
                .get("external")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || invocation.get("state").and_then(serde_json::Value::as_str)
                != Some("outcome_unknown")
            || invocation
                .get("dispatchCertainty")
                .and_then(serde_json::Value::as_str)
                != Some("possibly_dispatched")
            || invocation
                .get("outcome")
                .and_then(serde_json::Value::as_str)
                != Some("outcome_unknown")
            || invocation
                .get("errorCode")
                .and_then(serde_json::Value::as_str)
                != Some("mcp.tool_outcome_unknown")
            || invocation.get("isError").is_some()
            || invocation.get("rejectionReason").is_some()
            || invocation
                .get("durationMs")
                .and_then(serde_json::Value::as_u64)
                .is_none()
            || invocation
                .get("outputTruncated")
                .and_then(serde_json::Value::as_bool)
                != Some(false)
        {
            return Err("terminal MCP journal lifecycle is inconsistent".to_string());
        }
        let usage = usage_repository::load_usage_record_for_owner(
            &transaction,
            &record.run_id,
            conversation_id,
            assistant_message_id,
        )
        .map_err(storage_error)?
        .ok_or_else(|| "terminal MCP journal requires a durable usage record".to_string())?;
        if usage.status.as_deref() != Some("completed")
            || usage.completed_at.is_none()
            || usage.error.is_some()
        {
            return Err("terminal MCP journal usage is not completed".to_string());
        }

        let audit = agent_action_audit_repository::load_action_audit_record(
            &transaction,
            &record.action_id,
        )
        .map_err(storage_error)?
        .ok_or_else(|| "terminal MCP journal requires a durable action audit".to_string())?;
        if approval.approval_mode != crate::AgentMcpApprovalMode::Auto {
            return Err(
                "terminal MCP journal adoption requires an automatically authorized action"
                    .to_string(),
            );
        }
        // Older current producers mislabeled some hidden Auto journals as `manual`. Exact
        // terminal adoption is the one safe point where that projection can be normalized: the
        // frozen Auto action, dispatch boundary, and outcome-unknown result are already proven.
        let decision_source_is_valid =
            matches!(audit.decision_source.as_deref(), Some("auto" | "manual"));
        let normalized_decision_source = "auto";
        if audit.action_id != record.action_id
            || audit.run_id != record.run_id
            || audit.conversation_id != record.conversation_id
            || audit.assistant_message_id != record.assistant_message_id
            || audit.action_type != record.action_type
            || audit.tool_name != record.tool_name
            || audit.status != "approved"
            || audit.decision.as_deref() != Some("approved")
            || !decision_source_is_valid
            || audit.action_json != record.action_json
            || audit.file_change_result_json.is_some()
            || audit.command_result_json.is_some()
            || audit.completed_at.is_some()
            || audit.tool_result_json.is_some()
            || audit.error.is_some()
            || audit.created_at != record.created_at
            || audit.decided_at != Some(record.created_at)
            || audit.effective_permissions_json.is_none()
            || audit.path_scope.is_some()
            || audit.command_cwd_scope.is_some()
            || audit.blocked_reason.is_some()
        {
            return Err("terminal MCP journal durable audit identity is inconsistent".to_string());
        }

        let affected = transaction
            .execute(
                "UPDATE agent_pending_actions
                 SET status = 'failed',
                     target_status = 'failed',
                     action_json = '{}',
                     agent_input_json = '{}',
                     updated_at = ?3
                 WHERE action_id = ?1
                   AND status = ?2
                   AND target_status IS NULL
                   AND action_type = 'mcp_tool_call'",
                rusqlite::params![record.action_id, expected_status, updated_at],
            )
            .map_err(storage_error)?;
        if affected != 1 {
            return Err("terminal MCP journal adoption lost its pending status CAS".to_string());
        }
        resolve_pending_approval_notification_in_transaction(&transaction, &record, updated_at)?;
        let audit_affected = transaction
            .execute(
                "UPDATE agent_action_audit
                 SET status = 'failed',
                     action_json = '{}',
                     file_change_result_json = NULL,
                     command_result_json = NULL,
                     tool_result_json = NULL,
                     error = ?2,
                     blocked_reason = ?3,
                     completed_at = ?4,
                     decision_source = ?5
                 WHERE action_id = ?1
                   AND status = 'approved'
                   AND completed_at IS NULL",
                rusqlite::params![
                    record.action_id,
                    McpStartupActionTerminalOutcome::OutcomeUnknown.error_code(),
                    McpStartupActionTerminalOutcome::OutcomeUnknown.safe_reason(),
                    updated_at,
                    normalized_decision_source
                ],
            )
            .map_err(storage_error)?;
        if audit_affected != 1 {
            return Err("terminal MCP journal adoption lost its audit status CAS".to_string());
        }
        transaction
            .execute(
                "DELETE FROM mcp_approval_payload_envelopes WHERE action_id = ?1",
                [&record.action_id],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(true)
    }

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

}
