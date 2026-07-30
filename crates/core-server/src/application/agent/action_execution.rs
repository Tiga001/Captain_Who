use super::*;

mod execution_context;
mod file_authorization;

pub(super) use execution_context::{command_output_observer, AutoApprovedActionContext};
pub(super) use file_authorization::authorize_structured_file_write;

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
        let context = AutoApprovedActionContext::new(
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            skill_resources,
        )
        .with_notifications(notifications);
        Arc::new(move |action, cancellation_token| {
            let mut refreshed = context.clone();
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
                action_id: record.snapshot.action_id.clone(),
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

        // The original result is definitely absent. Persist an explicit durability failure that
        // embeds the original bounded execution evidence, so the model never sees a clean
        // rollback or loses provider output after the side effect has already been attempted.
        let attempt_error = attempt_errors.join("; retry: ");
        let persistence_failure = file_effect_audit_persistence_failure(
            &execution_result.call_id,
            &execution_result.tool,
            effect_type,
            "afterExecution",
            &attempt_error,
            Some(&execution_result),
        );
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
        if let Err(error) = authorize_structured_file_write(
            &agent_input,
            &action,
            FileWriteAuthorizationSource::Automatic,
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
            AgentProposedAction::Diff { diff } => {
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&agent_input) {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action_id = diff.id.clone();
                let execution = approved_patch_execution_for_input(&agent_input, &action_id, &diff);
                record_turn_file_change_best_effort(
                    &self.storage,
                    &agent_input,
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &action_id,
                    execution.file_change.as_ref(),
                );
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    AgentProposedAction::Diff { diff },
                    &execution.status,
                    execution.patch_result.as_ref(),
                    None,
                    Some(&execution.tool_result),
                    execution.tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deletion_lifecycle);
                Ok(execution.tool_result)
            }
            AgentProposedAction::Command { command } => {
                let effect_storage_id = pending_action_storage_id(&run_id, &command.id);
                let mut file_effect_guard =
                    self.register_file_effect(&agent_input, &run_id, &effect_storage_id)?;
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
                            file_effect_guard.mark_durably_settled();
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
                file_effect_guard.mark_effects_started();
                let output_observer = notifications.as_ref().map(|notifications| {
                    command_output_observer(&run_id, &command.id, notifications)
                });
                let command_result =
                    run_authorized_command_with_artifact_runtime_and_inputs_with_output_observer(
                        workspace_root.as_deref(),
                        &command,
                        permissions,
                        CommandAuthorizationSource::Automatic,
                        cancellation_token.clone(),
                        None,
                        self.artifact_runtime.as_deref(),
                        Some(&file_input_context),
                        output_observer,
                    )
                    .unwrap_or_else(|error| {
                        let policy_evaluation = error.policy_evaluation().cloned();
                        let artifact_observation = error.artifact_observation().cloned();
                        let mut result = failed_command_result(
                            &command_for_error,
                            error.to_string(),
                            policy_evaluation,
                        );
                        result.artifact_observation = artifact_observation;
                        result
                    });
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
                            file_effect_guard.mark_durably_settled();
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
                                file_effect_guard.mark_durably_settled();
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
                    file_effect_guard.mark_durably_settled();
                    return Ok(persistence_failure);
                }
                file_effect_guard.mark_durably_settled();
                Ok(tool_result)
            }
            AgentProposedAction::FileWrite { file_write } => {
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&agent_input) {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action = AgentProposedAction::FileWrite {
                    file_write: file_write.clone(),
                };
                let execution =
                    approved_file_write_execution(&self.storage, &agent_input, &file_write);
                record_turn_file_change_best_effort(
                    &self.storage,
                    &agent_input,
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &file_write.id,
                    execution.file_change.as_ref(),
                );
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    action,
                    &execution.status,
                    None,
                    None,
                    Some(&execution.tool_result),
                    execution.tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deletion_lifecycle);
                Ok(execution.tool_result)
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
        }
    }
}

/// Executes only the command frozen in the backend-owned pending-action snapshot.
///
/// The approval endpoint accepts an action id rather than a replacement command. Keeping snapshot
/// selection and the `ExplicitUser` authorization source together at this boundary prevents a
/// caller from turning approval of one command into execution of another.
#[cfg(test)]
pub(super) fn run_explicitly_approved_command_from_snapshot(
    record: &PendingActionRecord,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    run_explicitly_approved_command_from_snapshot_with_artifact_runtime(
        record,
        cancellation_token,
        action_cancel_flag,
        None,
        None,
    )
}

#[cfg(test)]
pub(super) fn run_explicitly_approved_command_from_snapshot_with_artifact_runtime(
    record: &PendingActionRecord,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    artifact_runtime: Option<&mycopilot_core::artifact_runtime::ArtifactRuntimeProvider>,
    file_inputs: Option<&AgentFileInputExecutionContext>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    run_explicitly_approved_command_from_snapshot_with_artifact_runtime_and_output_observer(
        record,
        cancellation_token,
        action_cancel_flag,
        artifact_runtime,
        file_inputs,
        None,
    )
}

pub(super) fn run_explicitly_approved_command_from_snapshot_with_artifact_runtime_and_output_observer(
    record: &PendingActionRecord,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    artifact_runtime: Option<&mycopilot_core::artifact_runtime::ArtifactRuntimeProvider>,
    file_inputs: Option<&AgentFileInputExecutionContext>,
    output_observer: Option<ProcessOutputObserver>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    let AgentProposedAction::Command { command } = &record.snapshot.action else {
        return Err("待审批操作不包含可执行命令。".to_string().into());
    };
    let workspace_root = workspace_root_optional(&record.agent_input);
    run_authorized_command_with_artifact_runtime_and_inputs_with_output_observer(
        workspace_root.as_deref(),
        command,
        permissions_from_input(&record.agent_input),
        CommandAuthorizationSource::ExplicitUser,
        cancellation_token,
        action_cancel_flag,
        artifact_runtime,
        file_inputs,
        output_observer,
    )
}

pub(super) fn cancelled_file_write_outcome_is_durable_or_unknown(
    storage: &StorageService,
    record: &PendingActionRecord,
) -> bool {
    let AgentProposedAction::FileWrite { file_write } = &record.snapshot.action else {
        return false;
    };
    match storage.get_agent_file_draft(&file_write.draft_id) {
        Ok(Some(draft)) => draft.status == "rejected",
        Ok(None) => false,
        // A storage read failure makes rollback unsafe: the rejection write may have committed.
        Err(_) => true,
    }
}
