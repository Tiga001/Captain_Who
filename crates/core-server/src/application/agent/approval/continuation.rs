impl AgentService {
    pub(super) fn queue_action_continuation(
        &self,
        run_id: &str,
        action_id: &str,
        decision_status: AgentApprovalDecisionStatus,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        // Root and projected approvals share this boundary. Feedback is optional; preserve
        // meaningful text verbatim while treating blank input like an omitted reason.
        let message = message.filter(|value| !value.trim().is_empty());
        #[cfg(test)]
        run_approval_decision_barrier_hook(action_id, decision_status);
        if decision_status == AgentApprovalDecisionStatus::Approved {
            if let Some(output) =
                self.try_resume_approved_mcp_action(run_id, action_id, notifications.clone())?
            {
                return Ok(output);
            }
        }
        let mut deletion_lifecycle = Some(
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let decided_at = now_ms();
        let (
            mut record,
            call,
            approved_process_guard,
            approved_materialization_guard,
            inline_continuation_guard,
            precommitted_builtin_rejection,
            provider_continuation_error,
        ) = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(storage_id) =
                resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
            else {
                return Err(format!(
                    "未找到待审批操作：runId={run_id}, actionId={action_id}"
                ));
            };
            let record = pending_actions
                .get(&storage_id)
                .expect("resolved pending action exists");
            let is_rejected_mcp = decision_status == AgentApprovalDecisionStatus::Rejected
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::McpToolCall { .. }
                        | AgentProposedAction::BuiltinMcpToolApproval { .. }
                );
            let is_recovered_approved_mcp_rejection = is_rejected_mcp
                && record.snapshot.status == PendingActionStatus::Approved
                && self
                    .startup_recoverable_mcp_approvals
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .contains(&storage_id);
            if record.snapshot.status != PendingActionStatus::Pending
                && !is_recovered_approved_mcp_rejection
            {
                return Err(format!("待审批操作已经处理：{action_id}"));
            }
            self.ensure_approval_predecessors_settled(record)?;
            let record = pending_actions
                .get_mut(&storage_id)
                .expect("resolved pending action exists");
            if deletion_lifecycle
                .as_ref()
                .expect("deletion lifecycle guard is held while preparing approval")
                .contains_input(&record.agent_input)
            {
                return Err("项目或会话正在移除，无法处理待审批操作。".to_string());
            }
            let provider_continuation_error = (decision_status
                == AgentApprovalDecisionStatus::Approved)
                .then(|| {
                    self.validate_provider_continuations_before_dispatch(record)
                        .err()
                })
                .flatten();
            let call = tool_call_for_pending_record(record)?;
            // A sensitive built-in rejection must win the durable decision CAS before its
            // process-only payload is mutated. Otherwise an approve thread can transition the
            // same pending row while Host rejection removes the frozen invocation, producing a
            // split-brain approval. Commit the value-free rejection ToolResult/trace first; the
            // target_status=rejected receipt makes every concurrent approve transition fail.
            let precommitted_builtin_rejection =
                if decision_status == AgentApprovalDecisionStatus::Rejected {
                    if let AgentProposedAction::BuiltinMcpToolApproval { approval } =
                        &record.snapshot.action
                    {
                        let tool_result = mycopilot_core::builtin_mcp_tool_rejected_result(
                            approval,
                            message.as_deref(),
                        );
                        let mut live_agent_input = record.agent_input.clone();
                        live_agent_input.approval_decision = Some(AgentApprovalDecision {
                            action_id: record.snapshot.action_id.clone(),
                            status: decision_status,
                            message: message.clone(),
                        });
                        live_agent_input.tool_continuation = Some(AgentToolContinuation {
                            call: call.clone(),
                            result: tool_result.clone(),
                        });
                        let mut persisted_agent_input = live_agent_input.clone();
                        if let Some(decision) = persisted_agent_input.approval_decision.as_mut() {
                            // User feedback is process-only model context. It can contain a
                            // password or other unlabelled secret and is never needed to prove
                            // the durable decision/ToolResult identity.
                            decision.message = None;
                        }
                        persisted_agent_input
                            .tool_continuation
                            .as_mut()
                            .expect("built-in rejection continuation was just populated")
                            .result =
                            mycopilot_core::builtin_capability_tool_result_persistence_projection(
                                &tool_result,
                            );
                        let completed_at = now_ms();
                        self.commit_rejected_mcp_receipt(
                            record,
                            &persisted_agent_input,
                            completed_at,
                            &notifications,
                        )?;
                        Some((live_agent_input, tool_result))
                    } else {
                        None
                    }
                } else {
                    None
                };
            if decision_status == AgentApprovalDecisionStatus::Approved
                && provider_continuation_error.is_none()
            {
                authorize_file_change_action(
                    &record.agent_input,
                    &record.snapshot.action,
                    FileChangeAuthorizationSource::ExplicitUser,
                )
                .map_err(|error| error.to_string())?;
            }
            let is_approved_process = decision_status == AgentApprovalDecisionStatus::Approved
                && provider_continuation_error.is_none()
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::Command { .. }
                        | AgentProposedAction::SkillScript { .. }
                        | AgentProposedAction::OfficeOperation { .. }
                        | AgentProposedAction::McpToolCall { .. }
                        | AgentProposedAction::BuiltinMcpToolApproval { .. }
                );
            let is_approved_skill_script = decision_status == AgentApprovalDecisionStatus::Approved
                && provider_continuation_error.is_none()
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::SkillScript { .. }
                );
            let is_approved_materialization = decision_status
                == AgentApprovalDecisionStatus::Approved
                && provider_continuation_error.is_none()
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::SkillMaterialization { .. }
                );
            let approved_process_guard = is_approved_process.then(|| {
                self.process_runs
                    .register(&record.storage_id, &record.snapshot.run_id)
            });
            let approved_materialization_guard = is_approved_materialization.then(|| {
                // The shared lifecycle lock is held, so direct registration is atomic with the
                // project/conversation tombstone check above and cannot race deletion.
                self.file_effects.register(
                    agent_input_project_id(&record.agent_input),
                    agent_input_conversation_id(&record.agent_input),
                    &record.snapshot.run_id,
                    &record.storage_id,
                )
            });
            let inline_continuation_guard = (!is_approved_process && !is_rejected_mcp).then(|| {
                // Synchronous decisions still queue an async model continuation. Register its
                // pre-spawn lease under the lifecycle lock so message deletion cannot pass
                // between durable action settlement and worker startup.
                self.process_runs
                    .register(&record.storage_id, &record.snapshot.run_id)
            });
            let execution_status = if is_approved_skill_script {
                PendingActionStatus::Executing
            } else if is_approved_process || is_approved_materialization {
                PendingActionStatus::Approved
            } else if is_rejected_mcp {
                // Rejection is definitely pre-dispatch. Keep either the original `pending`
                // state or a startup-recovered `approved` state until the paired audit,
                // target and ToolResult receipt commit atomically. `executing` remains reserved
                // for the conservative external-dispatch boundary.
                record.snapshot.status
            } else {
                PendingActionStatus::Executing
            };
            if execution_status != record.snapshot.status {
                if is_approved_skill_script {
                    let approved_audit = action_audit_record(
                        record,
                        Some("approved"),
                        "approved",
                        None,
                        None,
                        None,
                        None,
                        Some(decided_at),
                        None,
                    );
                    let active_child_wake = record
                        .agent_input
                        .context
                        .as_ref()
                        .and_then(|context| context.collaboration_identity.as_ref())
                        .map(|identity| {
                            ChildAgentFactory::new(Arc::clone(&self.storage))
                                .resolve_trusted_active_wake_by_identity(identity)
                                .map_err(|error| {
                                    format!("子 Agent 审批无法取得原 Wake 的执行权：{error}")
                                })
                        })
                        .transpose()?;
                    let child_wake = active_child_wake.as_ref().map(|active| {
                        (
                            active.spawn.initial_wake.wake_id.as_str(),
                            active.spawn.initial_wake.status,
                            active.claim_token.as_str(),
                        )
                    });
                    self.storage
                        .commit_pending_skill_script_approval_execution(
                            &record.storage_id,
                            &persisted_pending_agent_input_json(
                                &record.agent_input,
                                PendingActionStatus::Executing,
                            )?,
                            &approved_audit,
                            child_wake,
                            decided_at,
                        )?;
                } else {
                    self.persist_pending_dispatch_status(
                        record,
                        PendingActionStatus::Pending,
                        execution_status,
                    )?;
                }
                record.snapshot.status = execution_status;
            }
            (
                record.clone(),
                call,
                approved_process_guard,
                approved_materialization_guard,
                inline_continuation_guard,
                precommitted_builtin_rejection,
                provider_continuation_error,
            )
        };
        let mut approved_process_guard = approved_process_guard;
        let mut approved_materialization_guard = approved_materialization_guard;
        let mut inline_continuation_guard = inline_continuation_guard;

        let mut call = call;
        let is_mcp_action = matches!(
            record.snapshot.action,
            AgentProposedAction::McpToolCall { .. }
        );
        let is_builtin_mcp_action = matches!(
            record.snapshot.action,
            AgentProposedAction::BuiltinMcpToolApproval { .. }
        );
        if !is_mcp_action && !is_builtin_mcp_action {
            call.approval_status = match decision_status {
                AgentApprovalDecisionStatus::Approved => AgentApprovalStatus::Approved,
                AgentApprovalDecisionStatus::Rejected => AgentApprovalStatus::Rejected,
            };
        }
        if decision_status == AgentApprovalDecisionStatus::Rejected
            && !is_mcp_action
            && !is_builtin_mcp_action
        {
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && provider_continuation_error.is_none()
            && matches!(record.snapshot.action, AgentProposedAction::Command { .. })
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_command_execution(
                record,
                call,
                approved_process_guard.expect("approved command registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && provider_continuation_error.is_none()
            && matches!(
                record.snapshot.action,
                AgentProposedAction::SkillScript { .. }
            )
        {
            drop(deletion_lifecycle);
            return self.queue_skill_script_execution(
                record,
                call,
                approved_process_guard
                    .expect("approved Skill script registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && provider_continuation_error.is_none()
            && matches!(
                record.snapshot.action,
                AgentProposedAction::OfficeOperation { .. }
            )
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_office_operation_execution(
                record,
                call,
                approved_process_guard
                    .expect("approved Office operation registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && provider_continuation_error.is_none()
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
            )
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_mcp_tool_execution(
                record,
                call,
                approved_process_guard
                    .expect("approved MCP invocation registered under pending lock"),
                notifications,
            );
        }
        let mut builtin_mcp_grant_error = None;
        if decision_status == AgentApprovalDecisionStatus::Approved
            && provider_continuation_error.is_none()
            && is_builtin_mcp_action
        {
            let AgentProposedAction::BuiltinMcpToolApproval { approval } = &record.snapshot.action
            else {
                unreachable!("typed built-in MCP action was checked above");
            };
            let mut approved = (**approval).clone();
            approved.approval_status = AgentApprovalStatus::Approved;
            let grant = self
                .builtin_capabilities
                .as_ref()
                .ok_or_else(|| "Built-in capability Host is unavailable.".to_string())
                .and_then(|runtime| {
                    runtime
                        .approve_builtin_mcp_tool(&approved)
                        .map(|grant| (runtime, grant))
                        .map_err(|error| error.to_string())
                });
            match grant {
                Ok((runtime, grant)) => {
                    self.record_action_audit(
                        &record,
                        Some("approved"),
                        "approved",
                        None,
                        None,
                        None,
                        None,
                        Some(decided_at),
                        None,
                    );
                    drop(deletion_lifecycle);
                    let queue = self.queue_builtin_mcp_tool_execution(
                        record,
                        call,
                        grant.clone(),
                        approved_process_guard.take().expect(
                            "approved built-in MCP invocation registered under pending lock",
                        ),
                        notifications,
                    );
                    if queue.is_err() {
                        let _ = runtime
                            .revoke_builtin_mcp_tool_grant(&grant.grant_id, &grant.approval_id);
                    }
                    return queue;
                }
                Err(error) => {
                    builtin_mcp_grant_error = Some(error);
                    // This approval already won the durable pending -> approved decision CAS, but
                    // there is no process authority to dispatch. Convert its process lease into a
                    // synchronous continuation lease so the ordinary failed ToolResult is paired
                    // with the original frozen checkpoint before the agent resumes.
                    drop(approved_process_guard.take());
                    inline_continuation_guard = Some(
                        self.process_runs
                            .register(&record.storage_id, &record.snapshot.run_id),
                    );
                }
            }
        }

        let is_approved_materialization = decision_status == AgentApprovalDecisionStatus::Approved
            && provider_continuation_error.is_none()
            && matches!(
                record.snapshot.action,
                AgentProposedAction::SkillMaterialization { .. }
            );
        if is_approved_materialization {
            // The lease was registered atomically with the deletion-marker check above. Release
            // the global lifecycle mutex before resource restoration and filesystem I/O so a
            // deletion can install its marker and use the bounded FileEffectTracker drain.
            drop(deletion_lifecycle.take());
        }

        let mut continuation_message = message.clone();
        if decision_status == AgentApprovalDecisionStatus::Rejected {
            if let AgentProposedAction::SkillInstallation { installation } = &record.snapshot.action
            {
                let conversation_id =
                    record.snapshot.conversation_id.as_deref().ok_or_else(|| {
                        "Skill installation approval has no conversation identity.".to_string()
                    })?;
                // Rejection settles the durable approval even if its temporary installation
                // preparation has already expired or disappeared.
                if let Some(service) = self.skill_installation.as_ref() {
                    let _ = service.reject_action(
                        installation,
                        conversation_id,
                        &record.snapshot.run_id,
                    );
                }
            }
        }
        let mut builtin_activation_settlement = None;
        let mut execution = if let Some(error) = provider_continuation_error.as_deref() {
            ActionExecutionDecision {
                status: "failed".to_string(),
                final_pending_status: PendingActionStatus::Failed,
                file_change_result: None,
                file_change: None,
                committed_file_change_action: None,
                direct_file_change_finalization: None,
                tool_result: AgentToolResult {
                    exact_archive_file: None,
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: false,
                    result: Some(serde_json::json!({
                        "status": "failed",
                        "errorCode": "provider_continuation_unavailable",
                        "dispatchCertainty": "definitely_not_dispatched",
                        "retryable": false,
                    })),
                    error: Some(error.to_string()),
                },
            }
        } else if let Some(error) = builtin_mcp_grant_error.as_deref() {
            let AgentProposedAction::BuiltinMcpToolApproval { approval } = &record.snapshot.action
            else {
                unreachable!("built-in MCP grant errors only belong to typed built-in actions");
            };
            ActionExecutionDecision {
                status: "failed".to_string(),
                final_pending_status: PendingActionStatus::Failed,
                file_change_result: None,
                file_change: None,
                committed_file_change_action: None,
                direct_file_change_finalization: None,
                tool_result: AgentToolResult {
                    exact_archive_file: None,
                    call_id: approval.identity.call_id.clone(),
                    tool: approval.identity.model_name.clone(),
                    ok: false,
                    result: Some(serde_json::json!({
                        "schemaVersion": 1,
                        "type": "builtin_mcp_tool_approval",
                        "status": "failed",
                        "errorCode": "mcp.tool_approval_payload_unavailable",
                        "dispatchCertainty": "definitely_not_dispatched",
                        "retryable": false,
                        "contentOmitted": true,
                    })),
                    error: Some(format!(
                        "The sensitive built-in MCP Tool was not dispatched because its process authority was unavailable: {error}"
                    )),
                },
            }
        } else if decision_status == AgentApprovalDecisionStatus::Rejected && is_mcp_action {
            let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action else {
                unreachable!("typed MCP action was checked above");
            };
            let tool_result = mcp_tool_result_from_rejected_approval(approval, message.as_deref())
                .map_err(|error| error.to_string())?;
            continuation_message = tool_result
                .result
                .as_ref()
                .and_then(|value| value.get("userFeedback"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            ActionExecutionDecision {
                status: "rejected".to_string(),
                final_pending_status: PendingActionStatus::Rejected,
                file_change_result: None,
                file_change: None,
                committed_file_change_action: None,
                direct_file_change_finalization: None,
                tool_result,
            }
        } else if decision_status == AgentApprovalDecisionStatus::Rejected && is_builtin_mcp_action
        {
            let AgentProposedAction::BuiltinMcpToolApproval { approval } = &record.snapshot.action
            else {
                unreachable!("typed built-in MCP action was checked above");
            };
            if let Some(runtime) = self.builtin_capabilities.as_ref() {
                if runtime.reject_builtin_mcp_tool_approval(approval).is_err() {
                    // Durable user rejection is already authoritative. Cleanup is idempotent and
                    // must never turn it into a transport failure or reopen approval authority.
                    let _ = runtime.dismiss_builtin_mcp_tool_approval(approval);
                }
            }
            let tool_result = precommitted_builtin_rejection
                .as_ref()
                .map(|(_, result)| result.clone())
                .unwrap_or_else(|| {
                    mycopilot_core::builtin_mcp_tool_rejected_result(approval, message.as_deref())
                });
            continuation_message = message.clone();
            ActionExecutionDecision {
                status: "rejected".to_string(),
                final_pending_status: PendingActionStatus::Rejected,
                file_change_result: None,
                file_change: None,
                committed_file_change_action: None,
                direct_file_change_finalization: None,
                tool_result,
            }
        } else if let AgentProposedAction::BuiltinCapabilityActivation { approval } =
            &record.snapshot.action
        {
            let tool_result = if decision_status == AgentApprovalDecisionStatus::Rejected {
                mycopilot_core::builtin_capability_activation_rejected_result(
                    approval,
                    message.as_deref(),
                )
            } else {
                let mut approved = (**approval).clone();
                approved.approval_status = AgentApprovalStatus::Approved;
                // Waiting for a human decision has no deadline. The timestamp carried by the
                // pending card only bounded the original proposal; mint a fresh, short-lived
                // activation authorization when the user actually approves it. Grant lifetime
                // and all live policy/manifest checks remain unchanged.
                let approved_at =
                    u64::try_from(self.mcp_approval_now_ms().max(0)).unwrap_or_default() / 1_000;
                approved.created_at = approved_at;
                approved.expires_at = approved_at
                    .saturating_add(mycopilot_core::BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS);
                match self.builtin_capabilities.as_ref() {
                    Some(runtime) => match runtime.approve_activation(&approved) {
                        Ok(grant) => {
                            builtin_activation_settlement =
                                Some(BuiltinActivationSettlementGuard::new(
                                    runtime.clone(),
                                    grant.activation_id,
                                    approved.action_id.clone(),
                                ));
                            mycopilot_core::builtin_capability_activation_result(
                                &approved,
                                mycopilot_core::CapabilityActivationState::Active,
                                None,
                            )
                        }
                        Err(error) => AgentToolResult {
                            exact_archive_file: None,
                            call_id: approved.call_id.clone(),
                            tool: "activate_capability".to_string(),
                            ok: false,
                            result: Some(serde_json::json!({
                                "status": "revoked",
                                "capability": approved.capability_id,
                            })),
                            error: Some(error.to_string()),
                        },
                    },
                    None => AgentToolResult {
                        exact_archive_file: None,
                        call_id: approved.call_id.clone(),
                        tool: "activate_capability".to_string(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "status": "revoked",
                            "capability": approved.capability_id,
                        })),
                        error: Some("Builtin capability Host is unavailable.".to_string()),
                    },
                }
            };
            continuation_message = if decision_status == AgentApprovalDecisionStatus::Rejected {
                message.clone()
            } else {
                None
            };
            ActionExecutionDecision {
                status: if decision_status == AgentApprovalDecisionStatus::Rejected {
                    "rejected".to_string()
                } else if tool_result.ok {
                    // Keep the transport-level action status within the established approval
                    // vocabulary. The capability-specific ToolResult carries `status=active`;
                    // adding another wire enum here would create a second source of truth.
                    "approved".to_string()
                } else {
                    "failed".to_string()
                },
                final_pending_status: if decision_status == AgentApprovalDecisionStatus::Rejected {
                    PendingActionStatus::Rejected
                } else if tool_result.ok {
                    PendingActionStatus::Completed
                } else {
                    PendingActionStatus::Failed
                },
                file_change_result: None,
                file_change: None,
                committed_file_change_action: None,
                direct_file_change_finalization: None,
                tool_result,
            }
        } else if decision_status == AgentApprovalDecisionStatus::Approved {
            if let AgentProposedAction::SkillInstallation { installation } = &record.snapshot.action
            {
                let mut installation = (**installation).clone();
                installation.approval_status = AgentApprovalStatus::Approved;
                let tool_result = match (
                    record.snapshot.conversation_id.as_deref(),
                    self.skill_installation.as_ref(),
                ) {
                    (Some(conversation_id), Some(service)) => service.commit_approved(
                        &installation,
                        conversation_id,
                        &record.snapshot.run_id,
                    ),
                    _ => AgentToolResult {
                        exact_archive_file: None,
                        call_id: call.id.clone(),
                        tool: call.tool.clone(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "status": "failed",
                            "code": "installRefNotFound",
                            "recovery": "prepareAgain",
                        })),
                        error: Some(
                            "The Skill installation preparation is no longer available."
                                .to_string(),
                        ),
                    },
                };
                ActionExecutionDecision {
                    status: if tool_result.ok {
                        "installed".to_string()
                    } else {
                        "failed".to_string()
                    },
                    final_pending_status: if tool_result.ok {
                        PendingActionStatus::Completed
                    } else {
                        PendingActionStatus::Failed
                    },
                    file_change_result: None,
                    file_change: None,
                    committed_file_change_action: None,
                    direct_file_change_finalization: None,
                    tool_result,
                }
            } else if let AgentProposedAction::SkillMaterialization { materialization } =
                &record.snapshot.action
            {
                let mut materialization = materialization.clone();
                materialization.approval_status = AgentApprovalStatus::Approved;
                let tool_result = match self.restore_skill_resource_session(&record.agent_input) {
                    Ok(resources) => {
                        approved_materialization_guard
                            .as_mut()
                            .expect(
                                "approved Skill materialization registered under lifecycle lock",
                            )
                            .mark_effects_started();
                        self.execute_skill_materialization(
                            &record.agent_input,
                            &materialization,
                            resources.as_deref(),
                        )
                    }
                    Err(error) => AgentToolResult {
                        exact_archive_file: None,
                        call_id: materialization.id.clone(),
                        tool: "skills_materialize_resource".to_string(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "type": "skill_materialization",
                            "code": "snapshotUnavailable",
                            "recovery": "reactivateSkill",
                        })),
                        error: Some(error.to_string()),
                    },
                };
                ActionExecutionDecision {
                    status: if tool_result.ok {
                        "applied".to_string()
                    } else {
                        "failed".to_string()
                    },
                    final_pending_status: if tool_result.ok {
                        PendingActionStatus::Completed
                    } else {
                        PendingActionStatus::Failed
                    },
                    file_change_result: None,
                    file_change: None,
                    committed_file_change_action: None,
                    direct_file_change_finalization: None,
                    tool_result,
                }
            } else {
                action_execution_for_decision(
                    &self.storage,
                    &record,
                    &call,
                    decision_status,
                    message.as_deref(),
                )
            }
        } else {
            action_execution_for_decision(
                &self.storage,
                &record,
                &call,
                decision_status,
                message.as_deref(),
            )
        };
        if direct_file_change_outcome_unknown(&execution) {
            return Err(
                "无法确认文件修改结果；该操作保持恢复中，且未发布 ToolResult。请先检查文件状态。"
                    .to_string(),
            );
        }
        if let Some(committed_action) = execution.committed_file_change_action.take() {
            if let Err(error) =
                self.commit_pending_file_change_action(&mut record, committed_action)
            {
                eprintln!(
                    "failed to persist a manual Direct FileChange commit receipt; recovery remains pending: {error}"
                );
                return Err(
                    "文件修改的提交凭据尚未安全保存；该操作保持恢复中，且未发布 ToolResult。"
                        .to_string(),
                );
            }
        }
        if let Some(finalization) = execution.direct_file_change_finalization.take() {
            let finalized_action = finalize_direct_file_change_action(
                &record.snapshot.action,
                finalization,
            )
            .map_err(|_| {
                "Direct delete 清理尚未完成；该操作保持恢复中，且未发布 ToolResult。".to_string()
            })?;
            if let Err(error) =
                self.commit_pending_file_change_action(&mut record, finalized_action)
            {
                eprintln!(
                    "failed to persist a finalized manual Direct delete receipt; recovery remains pending: {error}"
                );
                return Err(
                    "Direct delete 清理凭据尚未安全保存；该操作保持恢复中，且未发布 ToolResult。"
                        .to_string(),
                );
            }
        }
        let mut final_pending_status = execution.final_pending_status;
        let mut tool_result = execution.tool_result.clone();
        let mut execution_status = execution.status.clone();
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::SkillInstallation { .. }
            )
        {
            let commit_may_have_succeeded = tool_result
                .result
                .as_ref()
                .and_then(|value| value.get("commitMayHaveSucceeded"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if tool_result.ok || commit_may_have_succeeded {
                if let (Some(installations), Some(workflow)) = (
                    self.skill_installation_service.as_deref(),
                    self.skill_installation_workflow.as_deref(),
                ) {
                    crate::transport::notify_skills_changed(
                        &self.storage,
                        &self.skills,
                        installations,
                        Some(workflow),
                        Some(&notifications),
                        if tool_result.ok {
                            mycopilot_protocol_rs::SkillsChangedReasonDto::Installed
                        } else {
                            mycopilot_protocol_rs::SkillsChangedReasonDto::CatalogChanged
                        },
                        None,
                    );
                }
            }
        }
        let is_rejected_mcp = decision_status == AgentApprovalDecisionStatus::Rejected
            && (is_mcp_action || is_builtin_mcp_action);
        let agent_input = if is_rejected_mcp {
            let (agent_input, needs_commit) = match precommitted_builtin_rejection.as_ref() {
                Some((agent_input, _)) => (agent_input.clone(), false),
                None => {
                    let mut agent_input = record.agent_input.clone();
                    agent_input.approval_decision = Some(AgentApprovalDecision {
                        action_id: record.snapshot.action_id.clone(),
                        status: decision_status,
                        message: continuation_message.clone(),
                    });
                    agent_input.tool_continuation = Some(AgentToolContinuation {
                        call: call.clone(),
                        result: tool_result.clone(),
                    });
                    (agent_input, true)
                }
            };
            if needs_commit {
                let mut persisted_agent_input = agent_input.clone();
                if is_builtin_mcp_action {
                    if let Some(decision) = persisted_agent_input.approval_decision.as_mut() {
                        decision.message = None;
                    }
                }
                if let Some(continuation) = persisted_agent_input.tool_continuation.as_mut() {
                    continuation.result = if is_builtin_mcp_action {
                        mycopilot_core::builtin_capability_tool_result_persistence_projection(
                            &continuation.result,
                        )
                    } else {
                        mycopilot_core::mcp_tool_result_persistence_projection(&continuation.result)
                    };
                }
                self.commit_rejected_mcp_receipt(
                    &record,
                    &persisted_agent_input,
                    now_ms(),
                    &notifications,
                )?;
            }
            self.claim_rejected_mcp_continuation(&record)?;
            // Only the durable status-CAS winner owns the pre-spawn lease. Registering before
            // arbitration would let a duplicate reject overwrite the winner's action-keyed guard
            // and unregister it when the losing guard drops.
            inline_continuation_guard = Some(
                self.process_runs
                    .register(&record.storage_id, &record.snapshot.run_id),
            );
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            if let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action {
                if let Ok(invocation) = mcp_tool_invocation_event(
                    approval,
                    McpToolInvocationEventUpdate {
                        state: AgentMcpToolInvocationState::Rejected,
                        dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
                        outcome: Some(AgentMcpToolInvocationOutcome::Rejected),
                        is_error: None,
                        error_code: Some("mcp.approval_rejected"),
                        duration_ms: None,
                        output_truncated: false,
                        result_size: None,
                        failure_stage: None,
                    },
                ) {
                    let _ = notifications.send(agent_event_notification(
                        AgentEvent::McpToolInvocationStateChanged {
                            run_id: record.snapshot.run_id.clone(),
                            invocation,
                        },
                    ));
                }
            }
            agent_input
        } else if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::FileChange { .. }
            )
        {
            let effect_kind = match &record.snapshot.action {
                AgentProposedAction::FileChange { file_change }
                    if file_change.inline_diff.is_none() =>
                {
                    "staged_file_change"
                }
                AgentProposedAction::FileChange { .. } => "direct_file_change",
                _ => unreachable!("FileChange approval branch matched a different action"),
            };
            match self.settle_manual_file_effect(
                &record,
                &call,
                final_pending_status,
                tool_result,
                effect_kind,
                &notifications,
            ) {
                ManualFileEffectSettlement::Committed {
                    agent_input,
                    tool_result: settled_result,
                    pending_status,
                } => {
                    let settled_file_change_result = file_change_result_for_audit(
                        &record,
                        &settled_result,
                    )?
                    .ok_or_else(|| {
                        "FileChange settlement is missing its typed terminal receipt.".to_string()
                    })?;
                    final_pending_status = pending_status;
                    tool_result = settled_result;
                    if settled_file_change_result.status
                        == mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown
                    {
                        execution_status = "outcome_unknown".to_string();
                    }
                    execution.file_change_result = Some(settled_file_change_result);
                    *agent_input
                }
                ManualFileEffectSettlement::CommittedAndAdvanced => {
                    return Err(
                        "FileChange receipt was already advanced by another continuation; duplicate continuation was stopped."
                            .to_string(),
                    );
                }
                ManualFileEffectSettlement::Unsettled => {
                    return Err(
                        "FileChange finished without a confirmed durable terminal receipt; inspect the target before retrying."
                            .to_string(),
                    );
                }
            }
        } else if is_approved_materialization {
            match self.settle_manual_file_effect(
                &record,
                &call,
                final_pending_status,
                tool_result,
                "skill_materialization",
                &notifications,
            ) {
                ManualFileEffectSettlement::Committed {
                    agent_input,
                    tool_result: settled_result,
                    pending_status,
                } => {
                    final_pending_status = pending_status;
                    tool_result = settled_result;
                    execution_status = if tool_result.ok {
                        "applied".to_string()
                    } else {
                        "failed".to_string()
                    };
                    approved_materialization_guard
                        .as_mut()
                        .expect("approved Skill materialization has an effect guard")
                        .mark_durably_settled();
                    *agent_input
                }
                ManualFileEffectSettlement::CommittedAndAdvanced => {
                    approved_materialization_guard
                        .as_mut()
                        .expect("approved Skill materialization has an effect guard")
                        .mark_durably_settled();
                    return Err(
                        "Skill materialization receipt was already advanced by another continuation; duplicate continuation was stopped."
                            .to_string(),
                    );
                }
                ManualFileEffectSettlement::Unsettled => {
                    return Err(
                        "Skill materialization finished without a confirmed durable terminal receipt; inspect state before retrying."
                            .to_string(),
                    );
                }
            }
        } else {
            self.record_action_audit(
                &record,
                Some(match decision_status {
                    AgentApprovalDecisionStatus::Approved => "approved",
                    AgentApprovalDecisionStatus::Rejected => "rejected",
                }),
                &execution.status,
                execution.file_change_result.as_ref(),
                None,
                Some(&tool_result),
                tool_result.error.as_deref(),
                Some(decided_at),
                Some(now_ms()),
            );
            self.persist_pending_target_status(&record, final_pending_status)?;

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
                status: decision_status,
                message: continuation_message.clone(),
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call: call.clone(),
                result: tool_result.clone(),
            });
            let mut persisted_agent_input = agent_input.clone();
            if is_builtin_mcp_action {
                if let Some(continuation) = persisted_agent_input.tool_continuation.as_mut() {
                    continuation.result =
                        mycopilot_core::builtin_capability_tool_result_persistence_projection(
                            &continuation.result,
                        );
                }
            }
            self.commit_trace_snapshot_with_continuation(
                &record,
                &persisted_agent_input,
                &notifications,
            )?;
            agent_input
        };
        if let Some(settlement) = builtin_activation_settlement.take() {
            // From this point the pending target and paired continuation are durably committed.
            // Before this boundary the guard removes the process-memory grant on every error path.
            settlement.commit();
        }
        let run_id = record.snapshot.run_id.clone();
        let inline_continuation_guard = inline_continuation_guard
            .expect("every synchronous approval decision owns a pre-spawn continuation lease");
        let continuation_cancel_flag = inline_continuation_guard.cancel_flag();
        let continuation_cancellation = AgentCancellationToken::new();
        self.register_cancellation(&run_id, continuation_cancellation.clone());
        // Inline diff/file-write actions keep the shared lifecycle lock until their target status
        // and paired ToolResult trace are durable. Materialization and asynchronous process paths
        // use a FileEffectGuard, so deletion can observe and drain them without an unbounded mutex
        // wait.
        drop(approved_materialization_guard);
        drop(deletion_lifecycle);
        let publish_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !publish_lifecycle.contains_input(&record.agent_input)
            && publish_inline_file_change_tool_result(
                &notifications,
                &record.snapshot.run_id,
                &record.snapshot.action,
                decision_status,
                &tool_result,
            )
        {
            // Approved Office operations return earlier and publish exactly one ToolResult from
            // their asynchronous executor. Rejected Office actions reach this synchronous path,
            // so publish the paired rejection result here just like FileChange.
            if matches!(
                record.snapshot.action,
                AgentProposedAction::FileChange { .. }
            ) {
                if let Some(file_change_result) = execution.file_change_result.as_ref() {
                    if let Ok(Some(draft)) = self
                        .storage
                        .get_agent_file_change(&file_change_result.transaction_id)
                    {
                        if let Ok(snapshot) = file_change_snapshot(&draft) {
                            let _ = notifications.send(agent_event_notification(
                                AgentEvent::FileChangeUpdated {
                                    run_id: record.snapshot.run_id.clone(),
                                    file_change: snapshot,
                                },
                            ));
                        }
                    }
                }
            }
        }
        drop(publish_lifecycle);

        let service = self.clone();
        let continuation_record = record.clone();
        tokio::spawn(async move {
            // Deletion may have cancelled the pre-spawn lease before this task was first polled.
            // Translate that signal without consulting the lifecycle mutex so the worker can
            // release its lease while deletion holds the marker and waits for a bounded drain.
            if continuation_cancel_flag.load(Ordering::SeqCst) {
                continuation_cancellation.cancel();
            }
            service
                .run_action_continuation(
                    continuation_record,
                    agent_input,
                    notifications,
                    final_pending_status,
                    Some(continuation_cancellation),
                )
                .await;
            drop(inline_continuation_guard);
        });

        let renderer_tool_result = if matches!(
            record.snapshot.action,
            AgentProposedAction::FileChange { .. } | AgentProposedAction::McpToolCall { .. }
        ) {
            None
        } else {
            Some(tool_result)
        };
        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: execution_status,
            file_change_result: execution.file_change_result,
            command_result: None,
            // FileChange and MCP each have a dedicated typed approval/lifecycle contract. Keep the
            // generic result response for built-ins and runtime extensions only.
            tool_result: renderer_tool_result,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }
}
