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
}
