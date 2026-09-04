impl AgentService {
    fn ensure_approval_predecessors_settled(
        &self,
        successor: &PendingActionRecord,
    ) -> Result<(), String> {
        let frozen_result_call_ids = successor
            .agent_input
            .resume_checkpoint
            .as_ref()
            .map(|checkpoint| {
                checkpoint
                    .conversation_trace_items
                    .iter()
                    .filter_map(|item| match item {
                        ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                            Some(call_id.clone())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if self
            .storage
            .pending_agent_action_has_unsettled_predecessor(
                &successor.storage_id,
                &frozen_result_call_ids,
            )?
        {
            return Err(UNSETTLED_APPROVAL_PREDECESSOR_MESSAGE.to_string());
        }
        Ok(())
    }

    pub(crate) fn list_root_projected_approvals(
        &self,
        root_conversation_id: &str,
    ) -> Result<Vec<ProjectedAgentApproval>, String> {
        let root = self
            .collaboration_authorizer
            .authorize_user_conversation_write(root_conversation_id)
            .map_err(|error| error.to_string())?;
        let Some(root) = root else {
            return Ok(Vec::new());
        };
        if root.parent_agent_id.is_some() {
            return Err("Approval projection owner must be a root Agent.".to_string());
        }
        let tree = self
            .collaboration_authorizer
            .visible_tree(&root.agent_id)
            .map_err(|error| error.to_string())?;
        let by_conversation = tree
            .into_iter()
            .filter(|node| node.parent_agent_id.is_some())
            .map(|node| (node.conversation_id.clone(), node))
            .collect::<HashMap<_, _>>();
        let startup_recoverable = self
            .startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut projected = pending_actions
            .values()
            .filter_map(|record| {
                if record.snapshot.status != PendingActionStatus::Pending
                    && !(record.snapshot.status == PendingActionStatus::Approved
                        && startup_recoverable.contains(&record.storage_id))
                {
                    return None;
                }
                let source = by_conversation.get(record.snapshot.conversation_id.as_deref()?)?;
                Some(ProjectedAgentApproval {
                    approval_id: record.storage_id.clone(),
                    root_agent_id: root.agent_id.clone(),
                    root_conversation_id: root.conversation_id.clone(),
                    source_agent_id: source.agent_id.clone(),
                    source_task_name: source.task_name.clone(),
                    source_task_path: source.task_path.clone(),
                    source_conversation_id: source.conversation_id.clone(),
                    action: record.snapshot.clone(),
                })
            })
            .collect::<Vec<_>>();
        projected.sort_by(|left, right| {
            left.action
                .created_at
                .cmp(&right.action.created_at)
                .then_with(|| left.approval_id.cmp(&right.approval_id))
        });
        Ok(projected)
    }

    /// Routes a root-card decision back to the exact child Run. The UI supplies no child/run/action
    /// identity; all of it is reloaded from the durable framed approval id.
    pub(crate) fn decide_root_projected_approval(
        &self,
        root_conversation_id: &str,
        approval_id: &str,
        decision: ProjectedApprovalDecision,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<ProjectedApprovalDecisionResult, String> {
        let approval_id = approval_id.trim();
        if approval_id.is_empty() || approval_id.len() > 512 || approval_id.contains('\0') {
            return Err("Invalid approvalId.".to_string());
        }
        if message
            .as_ref()
            .is_some_and(|message| message.len() > 16 * 1024 || message.contains('\0'))
        {
            return Err("Approval message must be NUL-free and at most 16384 bytes.".to_string());
        }
        // Validate the root before looking up the opaque approval capability. Every miss and
        // tree/project mismatch below deliberately returns the same safe error so this endpoint
        // cannot be used to enumerate approval ids owned by another root.
        self.collaboration_authorizer
            .authorize_root_conversation(root_conversation_id)
            .map_err(|_| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
        let in_memory = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(approval_id)
            .cloned();
        let Some(record) = in_memory else {
            let durable = self
                .storage
                .get_pending_agent_action(approval_id)?
                .ok_or_else(|| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
            let source_conversation_id = durable
                .conversation_id
                .as_deref()
                .ok_or_else(|| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
            let (_, source) = self
                .collaboration_authorizer
                .authorize_root_projection(root_conversation_id, source_conversation_id)
                .map_err(|_| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
            let public_action_id =
                serde_json::from_str::<AgentProposedAction>(&durable.action_json)
                    .map(|action| action_id_for_action(&action))
                    .unwrap_or_else(|_| approval_id.to_string());
            return Ok(ProjectedApprovalDecisionResult {
                approval_id: approval_id.to_string(),
                source_agent_id: source.agent_id,
                run_id: durable.run_id,
                action_id: public_action_id,
                status: durable.status,
                accepted: false,
            });
        };

        let source_conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .ok_or_else(|| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
        let (_, source) = self
            .collaboration_authorizer
            .authorize_root_projection(root_conversation_id, source_conversation_id)
            .map_err(|_| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        let current = record.snapshot.status;
        let actionable = current == PendingActionStatus::Pending
            || (current == PendingActionStatus::Approved
                && decision == ProjectedApprovalDecision::Approve
                && self
                    .startup_recoverable_mcp_approvals
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .contains(approval_id));
        if !actionable {
            return Ok(ProjectedApprovalDecisionResult {
                approval_id: approval_id.to_string(),
                source_agent_id: source.agent_id,
                run_id,
                action_id,
                status: pending_status_label(current).to_string(),
                accepted: false,
            });
        }

        let result = match decision {
            ProjectedApprovalDecision::Approve => self
                .queue_action_continuation(
                    &run_id,
                    &action_id,
                    AgentApprovalDecisionStatus::Approved,
                    None,
                    notifications,
                )
                .map(|output| (output.status, true)),
            ProjectedApprovalDecision::Reject => self
                .queue_action_continuation(
                    &run_id,
                    &action_id,
                    AgentApprovalDecisionStatus::Rejected,
                    message,
                    notifications,
                )
                .map(|output| (output.status, true)),
            ProjectedApprovalDecision::Cancel => self
                .cancel_action_internal(&run_id, &action_id)
                .map(|cancelled| {
                    (
                        if cancelled { "cancelled" } else { "unchanged" }.to_string(),
                        cancelled,
                    )
                }),
        };
        match result {
            Ok((status, accepted)) => Ok(ProjectedApprovalDecisionResult {
                approval_id: approval_id.to_string(),
                source_agent_id: source.agent_id,
                run_id,
                action_id,
                status,
                accepted,
            }),
            Err(error) => {
                // A simultaneous click may win between the in-memory check and the existing
                // pending-action CAS. Re-read durable state and return an idempotent snapshot.
                let durable = self.storage.get_pending_agent_action(approval_id)?;
                if let Some(durable) = durable.filter(|durable| durable.status != "pending") {
                    return Ok(ProjectedApprovalDecisionResult {
                        approval_id: approval_id.to_string(),
                        source_agent_id: source.agent_id,
                        run_id,
                        action_id,
                        status: durable.status,
                        accepted: false,
                    });
                }
                Err(error)
            }
        }
    }
    /// Host-internal cancellation for one trusted child Wake run. A live Runtime uses the normal
    /// run token, any handed-off command Session is interrupted explicitly, and a durable approval
    /// with no live owner is atomically cancelled through the existing pending-action settlement
    /// path. Pending-action arbitration still runs after signalling a live token because a Turn
    /// may have committed WaitingForApproval while its retiring Runtime token is briefly still
    /// registered; treating that token alone as completion would strand the durable approval.
    pub(crate) fn interrupt_agent_wake_run(
        &self,
        run_id: &str,
    ) -> Result<AgentRunCancellationOutcome, String> {
        let conversation_id = self.conversation_id_for_run(run_id);
        let mut cancellation = self.cancel_exact_run_execution(run_id, conversation_id.as_deref());
        let action_ids = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|record| record.snapshot.run_id == run_id)
            .filter(|record| {
                matches!(
                    record.snapshot.status,
                    PendingActionStatus::Pending
                        | PendingActionStatus::Approved
                        | PendingActionStatus::Executing
                )
            })
            .map(|record| record.snapshot.action_id.clone())
            .collect::<Vec<_>>();
        for action_id in action_ids {
            let action = self.cancel_action_internal_with_outcome(run_id, &action_id)?;
            cancellation.record_resource_cleanup(action.affected);
            cancellation.record_turn_termination(action.turn_termination_confirmed);
        }
        Ok(cancellation)
    }

    pub(super) fn validate_provider_continuations_before_dispatch(
        &self,
        record: &PendingActionRecord,
    ) -> Result<(), String> {
        let Some(checkpoint) = record.agent_input.resume_checkpoint.as_ref() else {
            return Ok(());
        };
        let has_provider_tool_calls = !checkpoint
            .assistant_turn_identity
            .tool_call_identities
            .is_empty();
        checkpoint
            .provider_protocol_key
            .validate_against_config(&checkpoint.provider_profile_config)
            .map_err(|error| {
                format!(
                    "provider_context_boundary_required: frozen Provider profile/key mismatch before approved action dispatch: {error}"
                )
            })?;
        let capabilities = mycopilot_core::resolve_provider_runtime_capabilities(
            &checkpoint.provider_protocol_key,
        )
        .map_err(|error| {
            format!(
                "provider_context_boundary_required: Provider runtime capability unavailable before approved action dispatch: {error}"
            )
        })?;
        let requires_provider_continuation = capabilities
            .classify_turn(
                has_provider_tool_calls,
                !checkpoint.provider_continuation_refs.is_empty(),
                checkpoint.provider_profile_config.reasoning_mode(),
            )
            .requires_exact_approval_refs();
        if checkpoint.provider_continuation_refs.is_empty() {
            return if requires_provider_continuation {
                Err("provider_continuation_missing: approved action was not dispatched".to_string())
            } else {
                Ok(())
            };
        }
        let conversation_id = record.snapshot.conversation_id.as_deref().ok_or_else(|| {
            "provider_continuation.invalid_approval_scope: approved action was not dispatched"
                .to_string()
        })?;
        let assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .filter(|assistant_message_id| !assistant_message_id.trim().is_empty())
            .ok_or_else(|| {
                "provider_continuation.invalid_approval_scope: approved action was not dispatched"
                    .to_string()
            })?;
        let run_id = record.snapshot.run_id.trim();
        if run_id.is_empty() {
            return Err(
                "provider_continuation.invalid_approval_scope: approved action was not dispatched"
                    .to_string(),
            );
        }
        self.provider_continuation_vault
            .as_deref()
            .ok_or_else(|| {
                "provider_continuation.credential_unavailable: approved action was not dispatched"
                    .to_string()
            })?
            .validate_approval_checkpoint_refs(
                conversation_id,
                assistant_message_id,
                run_id,
                &checkpoint.provider_protocol_key,
                &checkpoint.provider_continuation_refs,
                &checkpoint.assistant_turn_identity,
            )
            .map_err(|error| format!("{}: approved action was not dispatched", error.code()))
    }

    pub fn list_pending_actions(&self) -> Vec<PendingAgentActionSnapshot> {
        let pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let startup_recoverable_mcp_approvals = self
            .startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut snapshots = pending_actions
            .iter()
            .filter(|(storage_id, record)| {
                record.snapshot.status == PendingActionStatus::Pending
                    || (record.snapshot.status == PendingActionStatus::Approved
                        && startup_recoverable_mcp_approvals.contains(*storage_id))
            })
            .map(|(_, record)| record.snapshot.clone())
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| snapshot.created_at);
        snapshots
    }

    /// Legacy renderer approval list. Child approvals are intentionally absent: they are exposed
    /// only through `list_root_projected_approvals`, which carries authenticated source identity.
    pub fn list_user_pending_actions(&self) -> Vec<PendingAgentActionSnapshot> {
        let mut snapshots = self.list_pending_actions();
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            snapshots.extend(coordinator.list_pending());
            snapshots.sort_by_key(|snapshot| snapshot.created_at);
        }
        snapshots
            .into_iter()
            .filter(|snapshot| {
                let Some(conversation_id) = snapshot.conversation_id.as_deref() else {
                    return true;
                };
                self.collaboration_authorizer
                    .authorize_user_conversation_write(conversation_id)
                    .is_ok()
            })
            .collect()
    }

    pub fn read_file_change(
        &self,
        transaction_id: &str,
        observer_root_conversation_id: Option<&str>,
        offset: Option<usize>,
        max_chars: Option<usize>,
    ) -> Result<AgentFileChangeContentPage, String> {
        let change = self
            .storage
            .get_agent_file_change(transaction_id)?
            .ok_or_else(|| format!("未找到文件修改事务：{transaction_id}"))?;
        self.authorize_file_change_read(observer_root_conversation_id, &change.conversation_id)?;
        let snapshot = file_change_snapshot(&change)?;
        let (content, offset, next_offset, truncated) =
            paginate_chars(&change.content, offset, max_chars);
        Ok(AgentFileChangeContentPage {
            file_change: snapshot,
            content,
            offset,
            next_offset,
            truncated,
        })
    }

    pub fn get_file_change_diff(
        &self,
        transaction_id: &str,
        observer_root_conversation_id: Option<&str>,
        offset: Option<usize>,
        max_chars: Option<usize>,
    ) -> Result<AgentFileChangeDiffPage, String> {
        let change = self
            .storage
            .get_agent_file_change(transaction_id)?
            .ok_or_else(|| format!("未找到文件修改事务：{transaction_id}"))?;
        self.authorize_file_change_read(observer_root_conversation_id, &change.conversation_id)?;
        let diff = file_change_diff(&change);
        let (patch, offset, next_offset, truncated) = paginate_chars(&diff, offset, max_chars);
        Ok(AgentFileChangeDiffPage {
            transaction_id: change.id,
            patch,
            offset,
            next_offset,
            truncated,
        })
    }

    pub fn get_file_change_history_diff(
        &self,
        input: &AgentFileChangeHistoryDiffRequest,
    ) -> Result<AgentFileChangeHistoryDiffPage, String> {
        let conversation_id = input.conversation_id.as_str();
        let assistant_message_id = input.assistant_message_id.as_str();
        let run_id = input.run_id.as_str();
        let tool_call_id = input.tool_call_id.as_str();
        let storage_id = pending_action_storage_id(run_id, tool_call_id);
        let audit = self
            .storage
            .get_agent_action_audit(&storage_id)?
            .ok_or_else(|| "未找到已完成的文件修改记录。".to_string())?;

        if audit.action_id != storage_id
            || audit.run_id != run_id
            || audit.conversation_id.as_deref() != Some(conversation_id)
            || audit.assistant_message_id.as_deref() != Some(assistant_message_id)
            || audit.action_type != "file_change"
            || audit.tool_name != "apply_patch"
            || !matches!(
                audit.status.as_str(),
                "completed" | "failed" | "cancelled" | "rejected"
            )
            || audit.completed_at.is_none()
        {
            return Err("文件修改历史身份或状态无效。".to_string());
        }
        self.authorize_file_change_read(
            input.observer_root_conversation_id.as_deref(),
            conversation_id,
        )?;

        let action = serde_json::from_str::<AgentProposedAction>(&audit.action_json)
            .map_err(|_| "文件修改历史记录无效。".to_string())?;
        let AgentProposedAction::FileChange { file_change } = action else {
            return Err("文件修改历史记录类型无效。".to_string());
        };
        if file_change.id != tool_call_id
            || file_change.execution.source_call_id != tool_call_id
            || file_change.execution.run_id != run_id
            || file_change.execution.conversation_id != conversation_id
        {
            return Err("文件修改历史绑定身份无效。".to_string());
        }

        let patch = match (
            file_change.inline_diff,
            file_change.execution.staged_transaction_id.as_ref(),
        ) {
            (Some(inline_diff), None) => inline_diff.patch,
            (None, Some(_)) => {
                FileChangePlan::from_binding(&file_change.execution)
                    .map_err(|_| "文件修改历史 Diff 无效。".to_string())?
                    .diff
            }
            _ => return Err("文件修改历史模式无效。".to_string()),
        };
        let (patch, offset, next_offset, truncated) =
            paginate_chars(&patch, input.offset, input.max_chars);
        Ok(AgentFileChangeHistoryDiffPage {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            tool_call_id: tool_call_id.to_string(),
            patch,
            offset,
            next_offset,
            truncated,
        })
    }

    fn authorize_file_change_read(
        &self,
        observer_root_conversation_id: Option<&str>,
        file_change_conversation_id: &str,
    ) -> Result<(), String> {
        match observer_root_conversation_id {
            Some(root_conversation_id) => self
                .authorize_exact_child_observer_read(
                    root_conversation_id,
                    file_change_conversation_id,
                )
                .map(|_| ())
                .map_err(|error| error.to_string()),
            None => self
                .authorize_user_conversation_write(file_change_conversation_id)
                .map_err(|error| error.to_string()),
        }
    }

    #[cfg(test)]
    pub fn approve_action(
        &self,
        run_id: &str,
        action_id: &str,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        self.approve_action_with_scope(
            run_id,
            action_id,
            AgentApprovalScopeDto::SingleAction,
            notifications,
        )
        .map_err(|error| error.to_string())
    }

    pub fn approve_action_with_scope(
        &self,
        run_id: &str,
        action_id: &str,
        approval_scope: AgentApprovalScopeDto,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, AgentServiceError> {
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            if let Some(conversation_id) = coordinator.pending_conversation_id(run_id, action_id) {
                if approval_scope != AgentApprovalScopeDto::SingleAction {
                    return Err(
                        "Run-scoped approval is available only for apply_patch create or update."
                            .to_string()
                            .into(),
                    );
                }
                self.collaboration_authorizer
                    .authorize_user_conversation_write(&conversation_id)
                    .map_err(|error| error.to_string())?;
                return Ok(coordinator.approve(run_id, action_id)?.ok_or_else(|| {
                    "Browser risk approval changed while the decision was being committed."
                        .to_string()
                })?);
            }
        }
        self.authorize_user_pending_action(run_id, action_id)?;

        let decision_lock = self.approval_retry_lock(run_id, action_id);
        let _decision_guard = decision_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let is_file_change = {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
                .and_then(|storage_id| pending_actions.get(&storage_id))
                .is_some_and(|record| {
                    matches!(
                        record.snapshot.action,
                        AgentProposedAction::FileChange { .. }
                    )
                })
        };
        if is_file_change || approval_scope == AgentApprovalScopeDto::RemainingApplyPatchInRun {
            match self.prepare_file_change_approval_retry(run_id, action_id, approval_scope)? {
                FileChangeApprovalRetry::Replay(output) => return Ok(*output),
                FileChangeApprovalRetry::Proceed => {}
            }
        }

        Ok(self.queue_action_continuation(
            run_id,
            action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )?)
    }

    fn approval_retry_lock(&self, run_id: &str, action_id: &str) -> Arc<Mutex<()>> {
        let key = format!(
            "{:p}:{}",
            Arc::as_ptr(&self.storage),
            pending_action_storage_id(run_id, action_id)
        );
        let mut locks = APPROVAL_RETRY_LOCKS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(key, Arc::downgrade(&lock));
        lock
    }

    fn prepare_file_change_approval_retry(
        &self,
        run_id: &str,
        action_id: &str,
        approval_scope: AgentApprovalScopeDto,
    ) -> Result<FileChangeApprovalRetry, AgentServiceError> {
        let storage_id = pending_action_storage_id(run_id, action_id);
        let pending = self
            .storage
            .get_pending_agent_action(&storage_id)?
            .ok_or_else(|| "Pending action is unavailable or no longer actionable.".to_string())?;
        let Some(file_change) =
            exact_file_change_for_approval_retry(&pending, run_id, action_id, &storage_id)?
        else {
            if approval_scope == AgentApprovalScopeDto::RemainingApplyPatchInRun {
                return Err(
                    "Run-scoped approval is available only for apply_patch create or update."
                        .to_string()
                        .into(),
                );
            }
            return Ok(FileChangeApprovalRetry::Proceed);
        };

        let grant = self
            .storage
            .get_file_change_run_grant_for_pending_action(&storage_id)
            .map_err(file_change_run_grant_decision_error)?;
        if let Some(grant) = grant.as_ref() {
            grant.validate().map_err(|_| {
                "The durable approval scope is invalid; the action was not executed again."
                    .to_string()
            })?;
            if grant.granting_pending_action_id != storage_id || grant.run_id != run_id {
                return Err(
                    "The durable approval scope does not match this action; the action was not executed again."
                        .to_string()
                        .into(),
                );
            }
        }

        let has_remaining_scope = grant.is_some();
        let requests_remaining_scope =
            approval_scope == AgentApprovalScopeDto::RemainingApplyPatchInRun;
        if pending.status == "pending" {
            if requests_remaining_scope {
                if let Some(grant) = grant.as_ref() {
                    if grant.status != FileChangeRunGrantStatus::Pending {
                        return Err(approval_scope_conflict_message().into());
                    }
                } else {
                    self.persist_file_change_run_grant_intent(run_id, action_id)?;
                }
            } else if has_remaining_scope {
                // A SingleAction retry is not allowed to retire, narrow, or otherwise reinterpret
                // an already durable RemainingApplyPatchInRun choice.
                return Err(approval_scope_conflict_message().into());
            }
            return Ok(FileChangeApprovalRetry::Proceed);
        }

        if has_remaining_scope != requests_remaining_scope {
            return Err(approval_scope_conflict_message().into());
        }
        if matches!(pending.status.as_str(), "rejected" | "cancelled") {
            return Err(
                "This action already has a different terminal decision; it was not executed again."
                    .to_string()
                    .into(),
            );
        }
        if !matches!(
            pending.status.as_str(),
            "approved" | "executing" | "completed" | "failed"
        ) {
            return Err(
                "The durable approval state is invalid; the action was not executed again."
                    .to_string()
                    .into(),
            );
        }

        if let Some(output) = self.replay_terminal_file_change_approval(
            &pending,
            &file_change,
            run_id,
            action_id,
            &storage_id,
        )? {
            return Ok(FileChangeApprovalRetry::Replay(Box::new(output)));
        }
        Ok(FileChangeApprovalRetry::Replay(Box::new(
            in_flight_file_change_approval_output(run_id, action_id, &file_change),
        )))
    }

    fn replay_terminal_file_change_approval(
        &self,
        pending: &AgentPendingActionRecord,
        file_change: &mycopilot_core::AgentFileChangeProposal,
        run_id: &str,
        action_id: &str,
        storage_id: &str,
    ) -> Result<Option<AgentActionExecutionOutput>, String> {
        let audit = self
            .storage
            .get_agent_action_audit(storage_id)?
            .ok_or_else(|| {
                "The durable approval receipt is unavailable; the action was not executed again."
                    .to_string()
            })?;
        validate_file_change_approval_audit_identity(
            &audit,
            pending,
            file_change,
            run_id,
            action_id,
            storage_id,
        )?;
        if audit.status == "pending" && audit.decision.is_none() && audit.completed_at.is_none() {
            return Ok(None);
        }
        if audit.decision.as_deref() != Some("approved")
            || audit.decision_source.as_deref() != Some("manual")
        {
            return Err(
                "The durable approval decision differs from this retry; the action was not executed again."
                    .to_string(),
            );
        }
        let Some(result_json) = audit.file_change_result_json.as_deref() else {
            if audit.completed_at.is_none()
                && matches!(audit.status.as_str(), "approved" | "executing")
            {
                return Ok(None);
            }
            return Err(
                "The durable FileChange receipt is incomplete; the action was not executed again."
                    .to_string(),
            );
        };
        let result: AgentFileChangeResult = serde_json::from_str(result_json).map_err(|_| {
            "The durable FileChange receipt is invalid; the action was not executed again."
                .to_string()
        })?;
        validate_replayed_file_change_result(file_change, &result)?;
        let expected_pending_status = match result.status {
            mycopilot_core::AgentFileChangeResultStatus::Applied
            | mycopilot_core::AgentFileChangeResultStatus::AlreadyApplied => "completed",
            mycopilot_core::AgentFileChangeResultStatus::Failed
            | mycopilot_core::AgentFileChangeResultStatus::Conflict
            | mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown => "failed",
            mycopilot_core::AgentFileChangeResultStatus::Rejected
            | mycopilot_core::AgentFileChangeResultStatus::Aborted
            | mycopilot_core::AgentFileChangeResultStatus::Expired => {
                return Err(
                    "The durable FileChange decision differs from this approval retry; the action was not executed again."
                        .to_string(),
                )
            }
        };
        if audit.status != expected_pending_status
            || audit.completed_at.is_none()
            || pending
                .target_status
                .as_deref()
                .is_some_and(|status| status != expected_pending_status)
            || (matches!(pending.status.as_str(), "completed" | "failed")
                && pending.status != expected_pending_status)
        {
            return Err(
                "The durable FileChange lifecycle differs from its receipt; the action was not executed again."
                    .to_string(),
            );
        }
        let tool_result: AgentToolResult =
            serde_json::from_str(audit.tool_result_json.as_deref().ok_or_else(|| {
                "The durable FileChange ToolResult is missing; the action was not executed again."
                    .to_string()
            })?)
            .map_err(|_| {
                "The durable FileChange ToolResult is invalid; the action was not executed again."
                    .to_string()
            })?;
        if tool_result.call_id != action_id
            || tool_result.tool != "apply_patch"
            || tool_result.result.as_ref() != Some(&serde_json::to_value(&result).map_err(|_| {
                "The durable FileChange receipt could not be verified; the action was not executed again."
                    .to_string()
            })?)
            || tool_result.ok
                != matches!(
                    result.status,
                    mycopilot_core::AgentFileChangeResultStatus::Applied
                        | mycopilot_core::AgentFileChangeResultStatus::AlreadyApplied
                )
        {
            return Err(
                "The durable FileChange ToolResult differs from its receipt; the action was not executed again."
                    .to_string(),
            );
        }
        Ok(Some(file_change_approval_output(
            run_id,
            action_id,
            file_change_result_execution_status(result.status),
            result,
        )))
    }

    fn persist_file_change_run_grant_intent(
        &self,
        run_id: &str,
        action_id: &str,
    ) -> Result<(), AgentServiceError> {
        let record = {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let storage_id = resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
                .ok_or_else(|| {
                    "Only a pending apply_patch create or update can grant approval for the remaining Run."
                        .to_string()
                })?;
            pending_actions
                .get(&storage_id)
                .filter(|record| record.snapshot.status == PendingActionStatus::Pending)
                .cloned()
                .ok_or_else(|| {
                    "Only a pending apply_patch create or update can grant approval for the remaining Run."
                        .to_string()
                })?
        };
        let AgentProposedAction::FileChange { file_change } = &record.snapshot.action else {
            return Err(
                "Run-scoped approval is available only for apply_patch create or update."
                    .to_string()
                    .into(),
            );
        };
        if !matches!(
            file_change.operation,
            mycopilot_core::AgentFileChangeOperation::Create
                | mycopilot_core::AgentFileChangeOperation::Update
        ) || file_change.approval_status != AgentApprovalStatus::Required
            || file_change.execution.source_tool_name != "apply_patch"
            || file_change.execution.source_call_id != file_change.id
            || file_change.execution.run_id != run_id
            || record.snapshot.tool_name != "apply_patch"
            || record.storage_id != pending_action_storage_id(run_id, &file_change.id)
            || file_change.execution.validate().is_err()
        {
            return Err(
                "Run-scoped approval is available only for a valid pending apply_patch create or update."
                    .to_string()
                    .into(),
            );
        }
        let context = record
            .agent_input
            .context
            .as_ref()
            .ok_or_else(|| "The pending FileChange has no frozen Run context.".to_string())?;
        let conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .filter(|conversation_id| context.conversation_id.as_deref() == Some(*conversation_id))
            .ok_or_else(|| "The pending FileChange conversation owner is invalid.".to_string())?;
        let project_id = context.project_id.clone();
        let scope = derive_file_change_run_grant_scope(file_change, context)
            .map_err(|_| "The pending FileChange Run scope is invalid.".to_string())?;
        let created_at = now_ms();
        let grant = FileChangeRunGrantRecord {
            schema_version: FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
            grant_id: format!("fcgrant_{}", uuid::Uuid::new_v4()),
            revision: 0,
            status: FileChangeRunGrantStatus::Pending,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            project_id,
            scope_kind: scope.scope_kind,
            workspace_identity: scope.workspace_identity,
            canonical_scope_path: scope.canonical_scope_path,
            scope_directory_identity: scope.scope_directory_identity,
            granting_pending_action_id: record.storage_id,
            base_write_permission: scope.base_write_permission,
            granting_permission_revision: file_change.execution.permission_revision.clone(),
            granting_tool_set_revision: file_change.execution.tool_set_revision.clone(),
            granting_provider_wire_revision: file_change.execution.provider_wire_revision.clone(),
            apply_patch_contract_revision: APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION.to_string(),
            activation_result_digest: None,
            created_at,
            activated_at: None,
            inactive_at: None,
            revoked_at: None,
        };
        self.storage
            .create_pending_file_change_run_grant(&grant)
            .map_err(file_change_run_grant_decision_error)?;
        Ok(())
    }
}
