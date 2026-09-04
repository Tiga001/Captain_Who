impl StorageService {
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
        self.commit_pending_agent_action_audited_result_trace_with_model_context_internal(
            terminal_audit,
            expected_pending_status,
            target_status,
            trace,
            model_context_items,
            committed_at,
            false,
        )
    }

    /// Commits a ToolResult that will authorize a model continuation only if the exact Run has
    /// not been durably stopped. Terminal result bookkeeping uses the unguarded variant above.
    pub fn commit_pending_agent_action_audited_result_trace_for_continuation(
        &self,
        terminal_audit: &AgentActionAuditRecord,
        expected_pending_status: &str,
        target_status: &str,
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
        committed_at: i64,
    ) -> Result<AgentPendingActionResultCommitOutcome, String> {
        self.commit_pending_agent_action_audited_result_trace_with_model_context_internal(
            terminal_audit,
            expected_pending_status,
            target_status,
            trace,
            model_context_items,
            committed_at,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_pending_agent_action_audited_result_trace_with_model_context_internal(
        &self,
        terminal_audit: &AgentActionAuditRecord,
        expected_pending_status: &str,
        target_status: &str,
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
        committed_at: i64,
        require_active_run: bool,
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
        if require_active_run {
            let tree_stopped = transaction
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM agent_tree_run_stops WHERE run_id = ?1
                     )",
                    [&pending.run_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_error)?;
            if tree_stopped {
                return Err(
                    "stopped Agent Run cannot commit model-continuation authority".to_string(),
                );
            }
        }

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
