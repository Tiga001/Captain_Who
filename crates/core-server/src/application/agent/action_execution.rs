use super::*;

mod execution_context;
mod file_change_authorization;

pub(super) use execution_context::AutoApprovedActionContext;
pub(super) use file_change_authorization::authorize_file_change_action;

mod runners;
mod support;

pub(super) use support::ManualFileEffectSettlement;
use support::*;

impl AgentService {
    pub(super) fn host_action_executor(
        &self,
        agent_input: AgentChatInput,
        run_id: String,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
        skill_resources: Option<Arc<SkillResourceSession>>,
        notifications: CoreServerNotificationSender,
    ) -> AgentHostActionExecutor {
        let service = self.clone();
        // Core invokes Host actions on Tokio's blocking pool. Capture the owning runtime before
        // entering that pool so the synchronous Host executor can safely drive the one async MCP
        // invocation path without constructing or nesting another runtime.
        let runtime = tokio::runtime::Handle::try_current().ok();
        let context = AutoApprovedActionContext::new(
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            skill_resources,
        )
        .with_notifications(notifications);
        Arc::new(move |action, checkpoint, cancellation_token| {
            let mut refreshed = context.clone();
            if let AgentProposedAction::McpToolCall { approval } = action {
                let Some(checkpoint) = checkpoint else {
                    service.invalidate_mcp_pending_payload(&AgentProposedAction::McpToolCall {
                        approval: approval.clone(),
                    });
                    return Err(AgentError::structured(
                        "mcp.auto_checkpoint_missing",
                        "The automatic MCP invocation is missing its frozen run checkpoint.",
                        serde_json::json!({
                            "type": "mcp_tool",
                            "code": "autoCheckpointMissing",
                            "retryable": false,
                            "dispatchCertainty": "definitely_not_dispatched",
                        }),
                    ));
                };
                refreshed.agent_input =
                    agent_input_with_run_checkpoint(&refreshed.agent_input, &checkpoint);
                let Some(runtime) = runtime.as_ref() else {
                    service.invalidate_mcp_pending_payload(&AgentProposedAction::McpToolCall {
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
                return runtime.block_on(service.execute_auto_mcp_tool_action(
                    &refreshed,
                    approval,
                    cancellation_token,
                ));
            }
            if let AgentProposedAction::BuiltinMcpToolApproval { approval } = action {
                let Some(checkpoint) = checkpoint else {
                    service.invalidate_mcp_pending_payload(
                        &AgentProposedAction::BuiltinMcpToolApproval {
                            approval: approval.clone(),
                        },
                    );
                    return Err(AgentError::structured(
                        "builtin_mcp.auto_checkpoint_missing",
                        "The automatic built-in MCP invocation is missing its frozen run checkpoint.",
                        serde_json::json!({
                            "type": "builtin_mcp_tool_approval",
                            "code": "autoCheckpointMissing",
                            "retryable": false,
                            "dispatchCertainty": "definitely_not_dispatched",
                        }),
                    ));
                };
                refreshed.agent_input =
                    agent_input_with_run_checkpoint(&refreshed.agent_input, &checkpoint);
                let Some(runtime) = runtime.as_ref() else {
                    service.invalidate_mcp_pending_payload(
                        &AgentProposedAction::BuiltinMcpToolApproval {
                            approval: approval.clone(),
                        },
                    );
                    return Err(AgentError::structured(
                        "builtin_mcp.runtime_unavailable",
                        "The built-in MCP invocation runtime is unavailable.",
                        serde_json::json!({
                            "type": "builtin_mcp_tool_approval",
                            "code": "runtimeUnavailable",
                            "retryable": false,
                            "dispatchCertainty": "definitely_not_dispatched",
                        }),
                    ));
                };
                return runtime.block_on(service.execute_auto_builtin_mcp_tool_action(
                    &refreshed,
                    approval,
                    cancellation_token,
                ));
            }
            if matches!(action, AgentProposedAction::FileChange { .. }) {
                let Some(checkpoint) = checkpoint else {
                    return Err(AgentError::structured(
                        "agent.file_change_auto_checkpoint_missing",
                        "The automatic FileChange is missing its frozen run checkpoint.",
                        serde_json::json!({
                            "type": "file_change",
                            "code": "autoCheckpointMissing",
                            "outcome": "definitely_not_executed",
                            "retryable": false,
                        }),
                    ));
                };
                refreshed.agent_input =
                    agent_input_with_run_checkpoint(&refreshed.agent_input, &checkpoint);
            }
            service
                .refresh_agent_input_attachment_library(&mut refreshed.agent_input)
                .map_err(AgentError::new)?;
            service.execute_auto_approved_action(refreshed, action, cancellation_token)
        })
    }

    pub(super) fn settle_manual_file_effect(
        &self,
        record: &PendingActionRecord,
        call: &AgentToolCall,
        desired_pending_status: PendingActionStatus,
        execution_result: AgentToolResult,
        effect_type: &str,
        notifications: &CoreServerNotificationSender,
    ) -> ManualFileEffectSettlement {
        let run_id = &record.snapshot.run_id;
        let completed_at = now_ms();
        let build_input = |tool_result: &AgentToolResult| {
            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: if matches!(
                    record.snapshot.action,
                    AgentProposedAction::FileChange { .. }
                ) {
                    record.storage_id.clone()
                } else {
                    record.snapshot.action_id.clone()
                },
                status: AgentApprovalDecisionStatus::Approved,
                message: None,
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call: call.clone(),
                result: tool_result.clone(),
            });
            agent_input
        };

        let agent_input = build_input(&execution_result);
        let mut attempt_errors = Vec::new();
        for _ in 0..2 {
            match self.commit_audited_result_trace_with_continuation(
                record,
                &agent_input,
                desired_pending_status,
                None,
                completed_at,
                notifications,
            ) {
                Ok(()) => {
                    return ManualFileEffectSettlement::Committed {
                        agent_input: Box::new(agent_input),
                        tool_result: execution_result,
                        pending_status: desired_pending_status,
                    };
                }
                Err(error) => attempt_errors.push(error),
            }
        }

        match self.inspect_audited_result_trace_with_continuation(
            record,
            &agent_input,
            desired_pending_status,
            None,
            completed_at,
        ) {
            Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                return ManualFileEffectSettlement::Committed {
                    agent_input: Box::new(agent_input),
                    tool_result: execution_result,
                    pending_status: desired_pending_status,
                };
            }
            Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                return ManualFileEffectSettlement::CommittedAndAdvanced;
            }
            Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {}
            Ok(AgentPendingActionSettlementInspection::Diverged { component, reason }) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: format!(
                            "The file-producing action finished, but its terminal receipt is divergent; continuation stopped: {component}: {reason}"
                        ),
                        attempt_error: &attempt_errors.join("; retry: "),
                        inspection_error: Some(&reason),
                        execution_result: &execution_result,
                    },
                );
                return ManualFileEffectSettlement::Unsettled;
            }
            Err(inspection_error) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: format!(
                            "The file-producing action finished, but its terminal receipt could not be inspected; continuation stopped: {inspection_error}"
                        ),
                        attempt_error: &attempt_errors.join("; retry: "),
                        inspection_error: Some(&inspection_error),
                        execution_result: &execution_result,
                    },
                );
                return ManualFileEffectSettlement::Unsettled;
            }
        }

        if matches!(
            record.snapshot.action,
            AgentProposedAction::FileChange { .. }
        ) {
            if let Err(cleanup_error) =
                self.delete_uncommitted_continuation_archive(record, &agent_input, completed_at)
            {
                eprintln!(
                    "failed to retire an uncommitted FileChange history archive: {}",
                    bounded_audit_error(&cleanup_error)
                );
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: "The FileChange finished, but its uncommitted exact receipt could not be retired safely; continuation stopped."
                            .to_string(),
                        attempt_error: &attempt_errors.join("; retry: "),
                        inspection_error: None,
                        execution_result: &execution_result,
                    },
                );
                return ManualFileEffectSettlement::Unsettled;
            }
            if let Err(settlement_error) =
                self.settle_staged_file_change_audit_outcome_unknown(record, completed_at)
            {
                eprintln!(
                    "failed to settle the Staged FileChange durability status: {}",
                    bounded_audit_error(&settlement_error)
                );
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: "The FileChange finished, but its transaction status could not be settled safely; continuation stopped."
                            .to_string(),
                        attempt_error: &attempt_errors.join("; retry: "),
                        inspection_error: None,
                        execution_result: &execution_result,
                    },
                );
                return ManualFileEffectSettlement::Unsettled;
            }
        }

        // The original result is definitely absent. FileChange has a single strict terminal
        // receipt contract, so its durability fallback must remain an AgentFileChangeResult
        // instead of switching to the generic file-effect diagnostic shape. That typed
        // `outcome_unknown` receipt becomes authoritative for audit, Trace, notification, and RPC.
        // Other file-producing actions retain their own result contracts.
        let attempt_error = attempt_errors.join("; retry: ");
        let persistence_failure = match &record.snapshot.action {
            AgentProposedAction::FileChange { file_change } => {
                eprintln!(
                    "FileChange audit persistence failed after execution: {}",
                    bounded_audit_error(&attempt_error)
                );
                let result = file_change_result(
                    file_change,
                    mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown,
                    mycopilot_core::AgentFileChangeOutcome::OutcomeUnknown,
                    None,
                    Some("agent.apply_patch.outcome_unknown"),
                    Some("无法确认文件修改结果，请先检查文件当前状态。"),
                    Some("文件修改已经尝试执行，但终态凭据未能持久化；系统不会盲目重试或覆盖。"),
                );
                file_change_tool_result(&file_change.id, false, &result)
            }
            _ => file_effect_audit_persistence_failure(
                &execution_result.call_id,
                &execution_result.tool,
                effect_type,
                "afterExecution",
                &attempt_error,
                Some(&execution_result),
            ),
        };
        let failure_input = build_input(&persistence_failure);
        let failure_status = PendingActionStatus::Failed;
        let mut failure_errors = Vec::new();
        for _ in 0..2 {
            match self.commit_audited_result_trace_with_continuation(
                record,
                &failure_input,
                failure_status,
                None,
                completed_at,
                notifications,
            ) {
                Ok(()) => {
                    return ManualFileEffectSettlement::Committed {
                        agent_input: Box::new(failure_input),
                        tool_result: persistence_failure,
                        pending_status: failure_status,
                    };
                }
                Err(error) => failure_errors.push(error),
            }
        }

        let failure_error = failure_errors.join("; retry: ");
        match self.inspect_audited_result_trace_with_continuation(
            record,
            &failure_input,
            failure_status,
            None,
            completed_at,
        ) {
            Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                ManualFileEffectSettlement::Committed {
                    agent_input: Box::new(failure_input),
                    tool_result: persistence_failure,
                    pending_status: failure_status,
                }
            }
            Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                ManualFileEffectSettlement::CommittedAndAdvanced
            }
            Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_persistence_failed",
                        message: format!(
                            "The file-producing action finished, but its audit, target state and paired ToolResult trace were not persisted; continuation stopped: {failure_error}"
                        ),
                        attempt_error: &failure_error,
                        inspection_error: None,
                        execution_result: &execution_result,
                    },
                );
                ManualFileEffectSettlement::Unsettled
            }
            Ok(AgentPendingActionSettlementInspection::Diverged { component, reason }) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: format!(
                            "The file-producing action finished, but its failure receipt is divergent; continuation stopped: {component}: {reason}"
                        ),
                        attempt_error: &failure_error,
                        inspection_error: Some(&reason),
                        execution_result: &execution_result,
                    },
                );
                ManualFileEffectSettlement::Unsettled
            }
            Err(inspection_error) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: format!(
                            "The file-producing action finished, but its failure receipt could not be inspected; continuation stopped: {inspection_error}"
                        ),
                        attempt_error: &failure_error,
                        inspection_error: Some(&inspection_error),
                        execution_result: &execution_result,
                    },
                );
                ManualFileEffectSettlement::Unsettled
            }
        }
    }

    fn settle_staged_file_change_audit_outcome_unknown(
        &self,
        pending: &PendingActionRecord,
        completed_at: i64,
    ) -> Result<(), String> {
        let AgentProposedAction::FileChange { file_change } = &pending.snapshot.action else {
            return Ok(());
        };
        let binding = &file_change.execution;
        let Some(transaction_id) = binding.staged_transaction_id.as_deref() else {
            return Ok(());
        };
        if transaction_id != file_change.transaction_id {
            return Err("Staged FileChange transaction identity is invalid".to_string());
        }
        let mut transaction = self
            .storage
            .get_agent_file_change_for_owner(
                transaction_id,
                &binding.conversation_id,
                binding.project_id.as_deref(),
                &binding.run_id,
                "apply_patch",
            )?
            .ok_or_else(|| "Staged FileChange transaction is missing".to_string())?;
        let exact_identity = transaction.final_action_id.as_deref()
            == Some(file_change.id.as_str())
            && transaction.final_action_arguments_digest.as_deref()
                == Some(binding.source_args_digest.as_str())
            && transaction.final_permission_revision.as_deref()
                == Some(binding.permission_revision.as_str())
            && transaction.final_tool_set_revision.as_deref()
                == Some(binding.tool_set_revision.as_str())
            && transaction.final_provider_wire_revision.as_deref()
                == Some(binding.provider_wire_revision.as_str())
            && transaction.draft_revision
                == binding.staged_transaction_revision.unwrap_or(u64::MAX);
        if !exact_identity {
            return Err("Staged FileChange terminal identity is invalid".to_string());
        }
        if transaction.status == "outcome_unknown" {
            return Ok(());
        }
        if !matches!(
            transaction.status.as_str(),
            "applied" | "already_applied" | "conflict" | "failed"
        ) {
            return Err(format!(
                "Staged FileChange cannot become outcome_unknown from status {}",
                transaction.status
            ));
        }
        let expected_status = transaction.status.clone();
        let expected_revision = transaction.draft_revision;
        let expected_index = transaction.next_mutation_index;
        transaction.status = "outcome_unknown".to_string();
        transaction.stats_final = true;
        transaction.updated_at = completed_at;
        if self.storage.transition_agent_file_change(
            &expected_status,
            expected_revision,
            expected_index,
            &transaction,
        )? {
            Ok(())
        } else {
            Err("Staged FileChange outcome_unknown transition lost its exact CAS".to_string())
        }
    }

    pub(super) fn execute_auto_approved_action(
        &self,
        context: AutoApprovedActionContext,
        action: AgentProposedAction,
        cancellation_token: AgentCancellationToken,
    ) -> AgentResult<AgentToolResult> {
        let AutoApprovedActionContext {
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            skill_resources,
            notifications,
        } = context;
        cancellation_token.check()?;
        if self.is_agent_input_scope_deleting(&agent_input) {
            return Err(AgentError::cancelled());
        }
        let created_at = now_ms();
        match &action {
            AgentProposedAction::OfficeOperation { office_operation } => {
                match self.inspect_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(Some(outcome)) => {
                        if let Some(result) =
                            resolve_office_claim_outcome(office_operation, outcome)?
                        {
                            let effect_storage_id =
                                pending_action_storage_id(&run_id, &office_operation.id);
                            let mut file_effect_guard = self.register_file_effect(
                                &agent_input,
                                &run_id,
                                &effect_storage_id,
                            )?;
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(office_audit_persistence_failure(
                            office_operation,
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
            }
            AgentProposedAction::Command { command } => {
                match self.inspect_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(Some(outcome)) => {
                        if let Some(result) = resolve_command_claim_outcome(command, outcome)? {
                            let effect_storage_id = pending_action_storage_id(&run_id, &command.id);
                            let mut file_effect_guard = self.register_file_effect(
                                &agent_input,
                                &run_id,
                                &effect_storage_id,
                            )?;
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(command_audit_persistence_failure(
                            command,
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
            }
            AgentProposedAction::SkillMaterialization { materialization } => {
                match self.inspect_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(Some(outcome)) => {
                        if let Some(result) = resolve_file_effect_claim_outcome(
                            &materialization.id,
                            "skills_materialize_resource",
                            "skill_materialization",
                            outcome,
                        )? {
                            let effect_storage_id =
                                pending_action_storage_id(&run_id, &materialization.id);
                            let mut file_effect_guard = self.register_file_effect(
                                &agent_input,
                                &run_id,
                                &effect_storage_id,
                            )?;
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(file_effect_audit_persistence_failure(
                            &materialization.id,
                            "skills_materialize_resource",
                            "skill_materialization",
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
            }
            AgentProposedAction::SkillScript { script } => {
                match self.inspect_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(Some(outcome)) => {
                        if let Some(result) = resolve_file_effect_claim_outcome(
                            &script.id,
                            "skills_run_script",
                            "skill_script",
                            outcome,
                        )? {
                            let effect_storage_id = pending_action_storage_id(&run_id, &script.id);
                            let mut file_effect_guard = self.register_file_effect(
                                &agent_input,
                                &run_id,
                                &effect_storage_id,
                            )?;
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(file_effect_audit_persistence_failure(
                            &script.id,
                            "skills_run_script",
                            "skill_script",
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
            }
            _ => {}
        }
        if let Err(error) = authorize_file_change_action(
            &agent_input,
            &action,
            FileChangeAuthorizationSource::Automatic,
        ) {
            // A Host policy rejection is the result of this tool call, not a failure of the
            // agent transport. Office calls must stay paired with their original callId/tool so
            // the model loop and trace remain structurally valid.
            if matches!(&action, AgentProposedAction::OfficeOperation { .. }) {
                let tool_result = proposed_action_failure_result(&action, &error);
                if let Err(audit_error) = self.persist_auto_action_audit_if_absent(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    "rejected",
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    eprintln!(
                        "failed to write rejected Office auto action audit log: {audit_error}"
                    );
                }
                return Ok(tool_result);
            }
            return Err(error);
        }
        match action {
            AgentProposedAction::FileChange { file_change } => {
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&agent_input) {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action_id = file_change.id.clone();
                let prepared_action = AgentProposedAction::FileChange {
                    file_change: file_change.clone(),
                };
                let (Some(conversation_id), Some(assistant_message_id)) =
                    (conversation_id.as_deref(), assistant_message_id.as_deref())
                else {
                    return Err(AgentError::structured(
                        "agent.file_change_auto_owner_missing",
                        "The automatic FileChange is missing its durable conversation owner.",
                        serde_json::json!({
                            "type": "file_change",
                            "code": "ownerMissing",
                            "outcome": "definitely_not_executed",
                            "retryable": false,
                        }),
                    ));
                };
                let mut pending = self
                    .prepare_auto_file_change_action_journal_under_deletion_guard(
                        &run_id,
                        conversation_id,
                        assistant_message_id,
                        prepared_action,
                        agent_input.clone(),
                        &deletion_lifecycle,
                    )
                    .map_err(|error| {
                        eprintln!("automatic FileChange journal preparation failed: {error}");
                        AgentError::structured(
                            "agent.file_change_auto_journal_failed",
                            "The automatic FileChange could not persist its frozen recovery journal.",
                            serde_json::json!({
                                "type": "file_change",
                                "code": "journalUnavailable",
                                "outcome": "definitely_not_executed",
                                "retryable": false,
                            }),
                        )
                    })?;
                if !self
                    .claim_auto_file_change_dispatch(&mut pending)
                    .map_err(|_| direct_file_change_recovery_pending(&action_id))?
                {
                    return Err(direct_file_change_recovery_pending(&action_id));
                }
                let call = tool_call_for_pending_record(&pending)
                    .map_err(|_| direct_file_change_recovery_pending(&action_id))?;
                let mut execution = action_execution_for_decision(
                    &self.storage,
                    &pending,
                    &call,
                    AgentApprovalDecisionStatus::Approved,
                    None,
                );
                if direct_file_change_outcome_unknown(&execution) {
                    return Err(direct_file_change_recovery_pending(&action_id));
                }
                if let Some(committed_action) = execution.committed_file_change_action.take() {
                    self.commit_pending_file_change_action(&mut pending, committed_action)
                        .map_err(|_| direct_file_change_recovery_pending(&action_id))?;
                }
                if let Some(finalization) = execution.direct_file_change_finalization.take() {
                    let finalized_action = match finalize_direct_file_change_action(
                        &pending.snapshot.action,
                        finalization,
                    ) {
                        Ok(action) => action,
                        Err(_) => {
                            return Err(direct_file_change_recovery_pending(&action_id));
                        }
                    };
                    self.commit_pending_file_change_action(&mut pending, finalized_action)
                        .map_err(|_| direct_file_change_recovery_pending(&action_id))?;
                }
                let (fallback_notifications, _fallback_receiver) =
                    tokio::sync::mpsc::unbounded_channel();
                let notifications = notifications.as_ref().unwrap_or(&fallback_notifications);
                self.finalize_auto_file_change_action_journal(
                    &mut pending,
                    &execution,
                    notifications,
                )
                .map_err(|_| direct_file_change_recovery_pending(&action_id))?;
                drop(deletion_lifecycle);
                Ok(execution.tool_result)
            }
            AgentProposedAction::Command { command } => {
                let effect_storage_id = pending_action_storage_id(&run_id, &command.id);
                let mut file_effect_guard =
                    Some(self.register_file_effect(&agent_input, &run_id, &effect_storage_id)?);
                let action = AgentProposedAction::Command {
                    command: command.clone(),
                };
                match self.claim_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(outcome) => {
                        if let Some(result) = resolve_command_claim_outcome(&command, outcome)? {
                            // A prior attempt already published the authoritative terminal
                            // receipt. Clear any in-process unresolved marker restored by an
                            // earlier commit-unknown path before returning the replayed result.
                            file_effect_guard
                                .as_mut()
                                .expect("unstarted command retains its file-effect lease")
                                .mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Err(error) => {
                        return Ok(command_audit_persistence_failure(
                            &command,
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
                let workspace_root = workspace_root_optional(&agent_input);
                let permissions = permissions_from_input(&agent_input);
                let command_for_error = command.clone();
                let file_input_context = agent_file_input_execution_context(
                    &agent_input,
                    skill_resources.clone(),
                    Arc::clone(&self.storage),
                );
                file_effect_guard
                    .as_mut()
                    .expect("command retains its file-effect lease before Session start")
                    .mark_effects_started();
                let command_result = if let (Some(conversation_id), Some(assistant_message_id)) =
                    (conversation_id.as_deref(), assistant_message_id.as_deref())
                {
                    let cancellation_probe = cancellation_token.clone();
                    let launch = self.command_sessions.start(StartAgentCommandSession {
                        owner: CommandSessionOwner {
                            conversation_id: conversation_id.to_string(),
                            assistant_message_id: assistant_message_id.to_string(),
                            origin_run_id: run_id.clone(),
                            call_id: command.id.clone(),
                            project_id: agent_input_project_id(&agent_input)
                                .map(ToString::to_string),
                        },
                        workspace_root: workspace_root.as_deref(),
                        command: &command,
                        permissions,
                        authorization_source: CommandAuthorizationSource::Automatic,
                        approval_provenance: serde_json::json!({
                            "source": "automatic",
                            "approvalStatus": "approved",
                        }),
                        artifact_runtime: self.artifact_runtime.clone(),
                        office_engine: Some(self.office_engine.clone()),
                        file_inputs: Some(&file_input_context),
                        notifications: notifications.clone(),
                        cancellation_token: cancellation_token.clone(),
                        cancel_probe: Some(Arc::new(move || cancellation_probe.is_cancelled())),
                        file_effect_guard: &mut file_effect_guard,
                    });
                    match launch {
                        Ok(AgentCommandSessionLaunch::Running {
                            snapshot,
                            tool_result,
                            mut handoff_guard,
                        }) => {
                            // The Session lock is the ownership-transfer fence. Cancellation and
                            // the durable running receipt are decided while the Session is still
                            // Pending; only a confirmed receipt may move the process to Adopted.
                            let mut audit_definitely_uncommitted = false;
                            let handoff = handoff_guard.commit(
                                || cancellation_token.is_cancelled(),
                                || {
                                    let finalized = self.finalize_auto_action_execution_audit(
                                        &run_id,
                                        Some(conversation_id),
                                        Some(assistant_message_id),
                                        &agent_input,
                                        &action,
                                        "completed",
                                        None,
                                        &tool_result,
                                        None,
                                        created_at,
                                        now_ms(),
                                    );
                                    let Err(finalize_error) = finalized else {
                                        return Ok(());
                                    };
                                    match self
                                        .inspect_auto_action_execution_audit(
                                            &run_id,
                                            Some(conversation_id),
                                            Some(assistant_message_id),
                                            &agent_input,
                                            &action,
                                            created_at,
                                        )
                                        .map_err(|inspect_error| {
                                            format!(
                                                "automatic command handoff audit failed ({finalize_error}) and could not be inspected: {inspect_error}"
                                            )
                                        })
                                        .and_then(|outcome| {
                                            reconcile_command_audit_outcome(&command, outcome)
                                        })
                                    {
                                        Ok(CommandAuditReconciliation::Terminal(persisted))
                                            if agent_tool_results_match(
                                                &persisted,
                                                &tool_result,
                                            ) =>
                                        {
                                            // SQLite committed the exact running receipt even
                                            // though the caller observed a post-commit error.
                                            Ok(())
                                        }
                                        Ok(CommandAuditReconciliation::Executing) => {
                                            audit_definitely_uncommitted = true;
                                            Err(format!(
                                                "automatic command handoff audit is still executing after finalization failed: {finalize_error}"
                                            ))
                                        }
                                        Ok(CommandAuditReconciliation::Terminal(_)) => Err(
                                            format!(
                                                "automatic command handoff audit committed a different ToolResult after finalization failed: {finalize_error}"
                                            ),
                                        ),
                                        Err(error) => Err(error),
                                    }
                                },
                            );
                            match handoff {
                                Ok(AgentCommandHandoffOutcome::Adopted) => {
                                    return Ok(tool_result);
                                }
                                Ok(AgentCommandHandoffOutcome::CancelledBeforeCommit) => {
                                    let terminal = handoff_guard
                                        .abort_before_handoff()
                                        .map_err(|error| {
                                            AgentError::structured(
                                                "agent.command_session_pre_handoff_abort_failed",
                                                "The command was cancelled before its durable handoff, but the Host could not confirm process termination.",
                                                serde_json::json!({
                                                    "type": "command_session",
                                                    "code": "preHandoffTerminationUnconfirmed",
                                                    "sessionId": snapshot.session_id,
                                                    "effectsMayHaveOccurred": true,
                                                    "terminationConfirmed": false,
                                                    "error": bounded_audit_error(&error),
                                                }),
                                            )
                                        })?;
                                    let mut execution = terminal.execution;
                                    execution.cancelled = true;
                                    execution
                                }
                                Ok(AgentCommandHandoffOutcome::PersistenceFailed(error))
                                    if audit_definitely_uncommitted =>
                                {
                                    let terminal = handoff_guard
                                        .abort_before_handoff()
                                        .map_err(|abort_error| {
                                            AgentError::structured(
                                                "agent.command_session_pre_handoff_abort_failed",
                                                "The command handoff receipt was not committed, and the Host could not confirm process termination.",
                                                serde_json::json!({
                                                    "type": "command_session",
                                                    "code": "preHandoffTerminationUnconfirmed",
                                                    "sessionId": snapshot.session_id,
                                                    "effectsMayHaveOccurred": true,
                                                    "terminationConfirmed": false,
                                                    "auditError": bounded_audit_error(&error),
                                                    "terminationError": bounded_audit_error(&abort_error),
                                                }),
                                            )
                                        })?;
                                    let mut execution = terminal.execution;
                                    execution.error.get_or_insert_with(|| {
                                        format!(
                                            "Command Session was terminated before handoff because its running receipt was not durable: {error}"
                                        )
                                    });
                                    execution
                                }
                                Ok(AgentCommandHandoffOutcome::PersistenceFailed(error)) => {
                                    let termination = handoff_guard.abort_before_handoff();
                                    return Err(AgentError::structured(
                                        "agent.command_session_handoff_indeterminate",
                                        "The command Session was not handed off because its durable running receipt could not be confirmed.",
                                        serde_json::json!({
                                            "type": "command_session",
                                            "code": "handoffIndeterminate",
                                            "sessionId": snapshot.session_id,
                                            "effectsMayHaveOccurred": true,
                                            "terminationConfirmed": termination.is_ok(),
                                            "auditError": bounded_audit_error(&error),
                                            "terminationError": termination.as_ref().err().map(|value| bounded_audit_error(value)),
                                            "execution": termination.as_ref().ok().map(|value| &value.execution),
                                        }),
                                    ));
                                }
                                Err(error) => {
                                    let termination = handoff_guard.abort_before_handoff();
                                    return Err(AgentError::structured(
                                        "agent.command_session_handoff_failed",
                                        "The command Session ownership transfer could not be completed safely.",
                                        serde_json::json!({
                                            "type": "command_session",
                                            "code": "handoffFailed",
                                            "sessionId": snapshot.session_id,
                                            "effectsMayHaveOccurred": true,
                                            "terminationConfirmed": termination.is_ok(),
                                            "handoffError": bounded_audit_error(&error),
                                            "terminationError": termination.as_ref().err().map(|value| bounded_audit_error(value)),
                                        }),
                                    ));
                                }
                            }
                        }
                        Ok(AgentCommandSessionLaunch::Exited(terminal)) => terminal.execution,
                        Err(error) => failed_command_result(&command_for_error, error, None),
                    }
                } else {
                    // Every command is owned by one durable Session bound to the conversation and
                    // assistant message. Missing identity is a Host invariant violation and must
                    // fail before process creation.
                    failed_command_result(
                        &command_for_error,
                        "run_command 缺少持久会话身份，命令未启动。".to_string(),
                        None,
                    )
                };
                let command_succeeded = command_result.error.is_none()
                    && !command_result.timed_out
                    && !command_result.cancelled
                    && command_result.exit_code == Some(0);
                let tool_result = command_tool_result(&command.id, &command_result);
                let status = if command_succeeded {
                    "completed"
                } else {
                    "failed"
                };
                if let Err(error) = self.finalize_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    status,
                    Some(&command_result),
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    let reconciliation = self
                        .inspect_auto_action_execution_audit(
                            &run_id,
                            conversation_id.as_deref(),
                            assistant_message_id.as_deref(),
                            &agent_input,
                            &action,
                            created_at,
                        )
                        .map_err(|inspect_error| {
                            format!("failed to inspect command audit: {inspect_error}")
                        })
                        .and_then(|outcome| reconcile_command_audit_outcome(&command, outcome));
                    match reconciliation {
                        Ok(CommandAuditReconciliation::Terminal(persisted)) => {
                            // SQLite committed the first terminal receipt even though the caller
                            // observed an error. Durable state is authoritative; never replace a
                            // successful receipt with a manufactured persistence failure.
                            if let Some(guard) = file_effect_guard.as_mut() {
                                guard.mark_durably_settled();
                            }
                            return Ok(persisted);
                        }
                        Ok(CommandAuditReconciliation::Executing) => {}
                        Err(reconciliation_error) => {
                            let indeterminate = command_audit_finalization_indeterminate(
                                &command,
                                &error,
                                &reconciliation_error,
                                &command_result,
                            );
                            return Ok(indeterminate);
                        }
                    }

                    let persistence_failure = command_audit_persistence_failure(
                        &command,
                        "afterExecution",
                        &error,
                        Some(&command_result),
                    );
                    // A transient finalization failure gets one terminal retry. The immutable
                    // executing claim prevents a duplicate process execution, while persisting
                    // the original command result alongside the explicit durability failure.
                    if let Err(retry_error) = self.finalize_auto_action_execution_audit(
                        &run_id,
                        conversation_id.as_deref(),
                        assistant_message_id.as_deref(),
                        &agent_input,
                        &action,
                        "failed",
                        Some(&command_result),
                        &persistence_failure,
                        persistence_failure.error.as_deref(),
                        created_at,
                        now_ms(),
                    ) {
                        let reconciliation = self
                            .inspect_auto_action_execution_audit(
                                &run_id,
                                conversation_id.as_deref(),
                                assistant_message_id.as_deref(),
                                &agent_input,
                                &action,
                                created_at,
                            )
                            .map_err(|inspect_error| {
                                format!(
                                    "failed to inspect command audit after retry: {inspect_error}"
                                )
                            })
                            .and_then(|outcome| reconcile_command_audit_outcome(&command, outcome));
                        match reconciliation {
                            Ok(CommandAuditReconciliation::Terminal(persisted)) => {
                                if let Some(guard) = file_effect_guard.as_mut() {
                                    guard.mark_durably_settled();
                                }
                                return Ok(persisted);
                            }
                            Ok(CommandAuditReconciliation::Executing) => {
                                let indeterminate = command_audit_finalization_indeterminate(
                                    &command,
                                    &retry_error,
                                    "command audit remained executing after terminal retry",
                                    &command_result,
                                );
                                return Ok(indeterminate);
                            }
                            Err(reconciliation_error) => {
                                let indeterminate = command_audit_finalization_indeterminate(
                                    &command,
                                    &retry_error,
                                    &reconciliation_error,
                                    &command_result,
                                );
                                return Ok(indeterminate);
                            }
                        }
                    }
                    if let Some(guard) = file_effect_guard.as_mut() {
                        guard.mark_durably_settled();
                    }
                    return Ok(persistence_failure);
                }
                if let Some(guard) = file_effect_guard.as_mut() {
                    guard.mark_durably_settled();
                }
                Ok(tool_result)
            }
            AgentProposedAction::ToolCall { call } => Ok(AgentToolResult {
                exact_archive_file: None,
                call_id: call.id,
                tool: call.tool,
                ok: false,
                result: None,
                error: Some(
                    "自动批准执行器只支持结构化 apply_patch diff 和 run_command。".to_string(),
                ),
            }),
            AgentProposedAction::McpToolCall { approval } => Ok(AgentToolResult {
                exact_archive_file: None,
                call_id: approval.identity.call_id,
                tool: approval.identity.provenance.model_tool_name,
                ok: false,
                result: Some(serde_json::json!({
                    "type": "mcp_approval",
                    "code": "approvedInvocationRequired",
                    "retryable": false,
                })),
                error: Some(
                    "MCP tools can only execute through the one-time approved invocation path."
                        .to_string(),
                ),
            }),
            AgentProposedAction::SkillMaterialization { materialization } => {
                let effect_storage_id = pending_action_storage_id(&run_id, &materialization.id);
                let mut file_effect_guard =
                    self.register_file_effect(&agent_input, &run_id, &effect_storage_id)?;
                let action = AgentProposedAction::SkillMaterialization {
                    materialization: materialization.clone(),
                };
                match self.claim_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(outcome) => {
                        if let Some(result) = resolve_file_effect_claim_outcome(
                            &materialization.id,
                            "skills_materialize_resource",
                            "skill_materialization",
                            outcome,
                        )? {
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Err(error) => {
                        return Ok(file_effect_audit_persistence_failure(
                            &materialization.id,
                            "skills_materialize_resource",
                            "skill_materialization",
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
                file_effect_guard.mark_effects_started();
                let tool_result = self.execute_skill_materialization(
                    &agent_input,
                    &materialization,
                    skill_resources.as_deref(),
                );
                let status = if tool_result.ok {
                    "completed"
                } else {
                    "failed"
                };
                if let Err(error) = self.finalize_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    status,
                    None,
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    let reconciliation = self
                        .inspect_auto_action_execution_audit(
                            &run_id,
                            conversation_id.as_deref(),
                            assistant_message_id.as_deref(),
                            &agent_input,
                            &action,
                            created_at,
                        )
                        .map_err(|inspect_error| {
                            format!(
                                "failed to inspect Skill materialization audit: {inspect_error}"
                            )
                        })
                        .and_then(|outcome| {
                            reconcile_file_effect_audit_outcome(
                                &materialization.id,
                                "skills_materialize_resource",
                                outcome,
                            )
                        });
                    let reconciliation_error = match reconciliation {
                        Ok(FileEffectAuditReconciliation::Terminal(persisted)) => {
                            file_effect_guard.mark_durably_settled();
                            return Ok(persisted);
                        }
                        Ok(FileEffectAuditReconciliation::Executing) => {
                            "audit remained executing after terminal finalization".to_string()
                        }
                        Err(reconciliation_error) => reconciliation_error,
                    };
                    return Ok(file_effect_audit_persistence_failure(
                        &materialization.id,
                        "skills_materialize_resource",
                        "skill_materialization",
                        "afterExecution",
                        &format!("{error}; reconciliation: {reconciliation_error}"),
                        Some(&tool_result),
                    ));
                }
                file_effect_guard.mark_durably_settled();
                Ok(tool_result)
            }
            AgentProposedAction::SkillScript { script } => {
                let effect_storage_id = pending_action_storage_id(&run_id, &script.id);
                let mut file_effect_guard =
                    self.register_file_effect(&agent_input, &run_id, &effect_storage_id)?;
                let action = AgentProposedAction::SkillScript {
                    script: script.clone(),
                };
                match self.claim_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(outcome) => {
                        if let Some(result) = resolve_file_effect_claim_outcome(
                            &script.id,
                            "skills_run_script",
                            "skill_script",
                            outcome,
                        )? {
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Err(error) => {
                        return Ok(file_effect_audit_persistence_failure(
                            &script.id,
                            "skills_run_script",
                            "skill_script",
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
                file_effect_guard.mark_effects_started();
                let tool_result = self.execute_skill_script(
                    &agent_input,
                    &script,
                    skill_resources.as_deref(),
                    CommandAuthorizationSource::Automatic,
                    cancellation_token,
                    None,
                );
                let status = if tool_result.ok {
                    "completed"
                } else {
                    "failed"
                };
                if let Err(error) = self.finalize_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    status,
                    None,
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    let reconciliation = self
                        .inspect_auto_action_execution_audit(
                            &run_id,
                            conversation_id.as_deref(),
                            assistant_message_id.as_deref(),
                            &agent_input,
                            &action,
                            created_at,
                        )
                        .map_err(|inspect_error| {
                            format!("failed to inspect Skill script audit: {inspect_error}")
                        })
                        .and_then(|outcome| {
                            reconcile_file_effect_audit_outcome(
                                &script.id,
                                "skills_run_script",
                                outcome,
                            )
                        });
                    let reconciliation_error = match reconciliation {
                        Ok(FileEffectAuditReconciliation::Terminal(persisted)) => {
                            file_effect_guard.mark_durably_settled();
                            return Ok(persisted);
                        }
                        Ok(FileEffectAuditReconciliation::Executing) => {
                            "audit remained executing after terminal finalization".to_string()
                        }
                        Err(reconciliation_error) => reconciliation_error,
                    };
                    return Ok(file_effect_audit_persistence_failure(
                        &script.id,
                        "skills_run_script",
                        "skill_script",
                        "afterExecution",
                        &format!("{error}; reconciliation: {reconciliation_error}"),
                        Some(&tool_result),
                    ));
                }
                file_effect_guard.mark_durably_settled();
                Ok(tool_result)
            }
            AgentProposedAction::OfficeOperation { office_operation } => {
                let action = AgentProposedAction::OfficeOperation { office_operation };
                let AgentProposedAction::OfficeOperation { office_operation } = &action else {
                    unreachable!("the action was constructed as an Office operation")
                };
                let tool = office_tool_name(office_operation.prepared.request.document_kind);
                let effect_storage_id = pending_action_storage_id(&run_id, &office_operation.id);
                let mut file_effect_guard =
                    self.register_file_effect(&agent_input, &run_id, &effect_storage_id)?;
                let claim = self.claim_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                );
                match claim {
                    Ok(outcome) => {
                        if let Some(result) =
                            resolve_office_claim_outcome(office_operation, outcome)?
                        {
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Err(error) => {
                        return Ok(office_audit_persistence_failure(
                            office_operation,
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
                file_effect_guard.mark_effects_started();
                let tool_result = self.execute_office_operation(
                    &agent_input,
                    office_operation,
                    skill_resources.clone(),
                    cancellation_token,
                    None,
                );
                let status = if tool_result.ok {
                    "completed"
                } else {
                    "failed"
                };
                if let Err(error) = self.finalize_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    status,
                    None,
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    let reconciliation = self
                        .inspect_auto_action_execution_audit(
                            &run_id,
                            conversation_id.as_deref(),
                            assistant_message_id.as_deref(),
                            &agent_input,
                            &action,
                            created_at,
                        )
                        .map_err(|inspect_error| {
                            format!("failed to inspect Office audit: {inspect_error}")
                        })
                        .and_then(|outcome| {
                            reconcile_file_effect_audit_outcome(&office_operation.id, tool, outcome)
                        });
                    if let Ok(FileEffectAuditReconciliation::Terminal(persisted)) = reconciliation {
                        file_effect_guard.mark_durably_settled();
                        return Ok(persisted);
                    }
                    return Ok(office_audit_persistence_failure(
                        office_operation,
                        "afterExecution",
                        &error,
                        Some(&tool_result),
                    ));
                }
                file_effect_guard.mark_durably_settled();
                Ok(tool_result)
            }
            AgentProposedAction::SkillInstallation { installation } => {
                let Some(service) = self.skill_installation.as_ref() else {
                    return Err(AgentError::new(
                        "The Skill installation Host service is unavailable.",
                    ));
                };
                let conversation_id = conversation_id.as_deref().ok_or_else(|| {
                    AgentError::new("Skill installation requires a conversation identity.")
                })?;
                Ok(service.commit_approved(&installation, conversation_id, &run_id))
            }
            AgentProposedAction::BuiltinCapabilityActivation { approval } => {
                let permissions = permissions_from_input(&agent_input);
                if permissions.builtin_execution
                    != mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove
                    || approval.approval_status != AgentApprovalStatus::Approved
                    || approval.run_id != run_id
                {
                    return Err(AgentError::new(
                        "内置能力激活未获得当前任务的自动批准权限。",
                    ));
                }
                let runtime = self
                    .builtin_capabilities
                    .as_ref()
                    .ok_or_else(|| AgentError::new("Builtin capability Host is unavailable."))?;
                let action = AgentProposedAction::BuiltinCapabilityActivation {
                    approval: approval.clone(),
                };
                match self.claim_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(AgentActionAuditExecutionClaimOutcome::Claimed) => {}
                    Ok(
                        AgentActionAuditExecutionClaimOutcome::AlreadyClaimed { .. }
                        | AgentActionAuditExecutionClaimOutcome::IdentityConflict { .. },
                    ) => {
                        return Err(AgentError::structured(
                            "builtin_capability.auto_activation_already_claimed",
                            "The automatic built-in capability activation was already claimed and was not replayed.",
                            serde_json::json!({
                                "type": "builtin_capability_activation",
                                "code": "autoActivationAlreadyClaimed",
                                "retryable": false,
                            }),
                        ));
                    }
                    Err(_) => {
                        return Err(AgentError::structured(
                            "builtin_capability.auto_activation_audit_unavailable",
                            "The automatic built-in capability activation could not establish its durable execution receipt.",
                            serde_json::json!({
                                "type": "builtin_capability_activation",
                                "code": "autoActivationAuditUnavailable",
                                "retryable": false,
                            }),
                        ));
                    }
                }
                let (grant, mut tool_result, mut status) =
                    match runtime.approve_activation(&approval) {
                        Ok(grant) => (
                            Some(grant),
                            mycopilot_core::builtin_capability_activation_result(
                                &approval,
                                mycopilot_core::CapabilityActivationState::Active,
                                None,
                            ),
                            "completed",
                        ),
                        Err(error) => (
                            None,
                            AgentToolResult {
                                exact_archive_file: None,
                                call_id: approval.call_id.clone(),
                                tool: "activate_capability".to_string(),
                                ok: false,
                                result: Some(serde_json::json!({
                                    "status": "revoked",
                                    "capability": approval.capability_id,
                                })),
                                error: Some(error.to_string()),
                            },
                            "failed",
                        ),
                    };
                if cancellation_token.is_cancelled() {
                    if let Some(grant) = grant.as_ref() {
                        let _ =
                            runtime.revoke_activation(&grant.activation_id, &approval.action_id);
                    }
                    tool_result = AgentToolResult {
                        exact_archive_file: None,
                        call_id: approval.call_id.clone(),
                        tool: "activate_capability".to_string(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "status": "revoked",
                            "capability": approval.capability_id,
                        })),
                        error: Some("agent run 已取消。".to_string()),
                    };
                    status = "cancelled";
                }
                let finalize = self.finalize_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    status,
                    None,
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                if finalize.is_err() {
                    let reconciled = self
                        .inspect_auto_action_execution_audit(
                            &run_id,
                            conversation_id.as_deref(),
                            assistant_message_id.as_deref(),
                            &agent_input,
                            &action,
                            created_at,
                        )
                        .ok()
                        .flatten()
                        .is_some_and(|outcome| match outcome {
                            AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
                                status: persisted_status,
                                tool_result_json: Some(persisted_result),
                            } => {
                                persisted_status == status
                                    && serde_json::from_str::<AgentToolResult>(&persisted_result)
                                        .is_ok_and(|persisted| {
                                            agent_tool_results_match(&persisted, &tool_result)
                                        })
                            }
                            _ => false,
                        });
                    if !reconciled {
                        if let Some(grant) = grant.as_ref() {
                            let _ = runtime
                                .revoke_activation(&grant.activation_id, &approval.action_id);
                        }
                        return Err(AgentError::structured(
                            "builtin_capability.auto_activation_receipt_unavailable",
                            "The automatic built-in capability activation did not produce a durable terminal receipt; its process grant was revoked.",
                            serde_json::json!({
                                "type": "builtin_capability_activation",
                                "code": "autoActivationReceiptUnavailable",
                                "retryable": false,
                            }),
                        ));
                    }
                }
                Ok(tool_result)
            }
            AgentProposedAction::BuiltinMcpToolApproval { .. } => Err(AgentError::new(
                "内置 MCP 敏感 Tool 必须经过原调用的持久化用户审批。",
            )),
            AgentProposedAction::BrowserRiskApproval { .. } => Err(AgentError::new(
                "浏览器风险批准必须由进程内 authorization coordinator 结算。",
            )),
        }
    }
}

pub(super) fn cancelled_staged_file_change_outcome_is_durable_or_unknown(
    storage: &StorageService,
    record: &PendingActionRecord,
) -> bool {
    let AgentProposedAction::FileChange { file_change } = &record.snapshot.action else {
        return false;
    };
    let Some(transaction_id) = file_change.execution.staged_transaction_id.as_deref() else {
        return false;
    };
    match storage.get_agent_file_change(transaction_id) {
        Ok(Some(draft)) => draft.status == "aborted",
        Ok(None) => false,
        // A storage read failure makes rollback unsafe: the abort write may have committed.
        Err(_) => true,
    }
}
