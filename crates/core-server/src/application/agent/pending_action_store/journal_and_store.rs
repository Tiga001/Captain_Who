impl AgentService {
    /// Persists the hidden pre-dispatch journal for one automatically authorized MCP call.
    ///
    /// The row starts at `approved`, so it is never published through the pending-approval API.
    /// Only a subsequent durable `approved -> executing` CAS permits the transport invocation.
    pub(super) fn prepare_auto_mcp_action_journal(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
    ) -> Result<PendingActionRecord, String> {
        let (action_id, call_id) = match &action {
            AgentProposedAction::McpToolCall { approval }
                if approval.approval_mode == mycopilot_core::AgentMcpApprovalMode::Auto
                    && approval.call.approval_status == AgentApprovalStatus::Approved =>
            {
                (
                    approval.identity.action_id.clone(),
                    approval.identity.call_id.clone(),
                )
            }
            AgentProposedAction::BuiltinMcpToolApproval { approval }
                if approval.approval_status == AgentApprovalStatus::Approved =>
            {
                (
                    approval.identity.action_id.clone(),
                    approval.identity.call_id.clone(),
                )
            }
            _ => {
                return Err(
                    "automatic MCP journal requires a Host-authorized typed MCP action".to_string(),
                );
            }
        };
        let agent_input = bind_pending_provider_configuration(&self.storage, agent_input)?;
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deletion_lifecycle.contains_input(&agent_input) {
            return Err("项目或会话正在移除，无法启动 MCP 操作。".to_string());
        }
        if !pending_action_binding_matches_for_auto_journal(run_id, None, &action, &agent_input) {
            return Err("automatic MCP journal frozen identity is inconsistent".to_string());
        }

        let storage_id = pending_action_storage_id(run_id, &action_id);
        let record = PendingActionRecord {
            storage_id: storage_id.clone(),
            snapshot: PendingAgentActionSnapshot {
                action_id,
                run_id: run_id.to_string(),
                conversation_id: normalized_optional(conversation_id),
                assistant_message_id: normalized_optional(assistant_message_id),
                action_type: action_type_for_action(&action).to_string(),
                tool_name: tool_name_for_action(&action),
                tool_call_id: Some(call_id),
                action,
                created_at: now_ms(),
                status: PendingActionStatus::Approved,
            },
            agent_input,
        };

        {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if pending_actions.contains_key(&storage_id) {
                return Err("automatic MCP invocation journal already exists".to_string());
            }
        }
        let outcome = self.persist_pending_action(&record)?;
        if outcome != PendingActionStoreOutcome::Inserted {
            return Err(
                "automatic MCP invocation was already journaled and will not be replayed"
                    .to_string(),
            );
        }
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match pending_actions.entry(storage_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(record.clone());
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err("automatic MCP invocation journal raced another dispatch".to_string());
            }
        }
        drop(pending_actions);
        let audit_result = if matches!(
            &record.snapshot.action,
            AgentProposedAction::McpToolCall { .. }
        ) {
            self.persist_auto_external_mcp_journal_audit(&record)
        } else {
            self.persist_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(record.snapshot.created_at),
                None,
            )
        };
        if audit_result.is_err() {
            let _ = self.settle_auto_mcp_action_journal(
                &record,
                McpAutoActionJournalTerminalOutcome::Failed,
                None,
            );
            return Err(
                "automatic MCP invocation journal audit could not be persisted".to_string(),
            );
        }
        drop(deletion_lifecycle);
        Ok(record)
    }

    /// Claims the durable possibly-dispatched boundary immediately before transport invocation.
    pub(super) fn claim_auto_mcp_dispatch(
        &self,
        record: &mut PendingActionRecord,
    ) -> Result<(), String> {
        if record.snapshot.status != PendingActionStatus::Approved {
            return Err("automatic MCP journal is not ready for dispatch".to_string());
        }
        self.persist_pending_dispatch_status(
            record,
            PendingActionStatus::Approved,
            PendingActionStatus::Executing,
        )?;
        record.snapshot.status = PendingActionStatus::Executing;
        Ok(())
    }

    /// Persists the hidden, resumable FileChange journal for an automatically approved action.
    ///
    /// This is the same current Pending Action shape used by manual approval. It is intentionally
    /// not published to the approval UI, but still owns the exact ToolCall, frozen checkpoint,
    /// normalized runtime binding, and canonical pending id before any filesystem side effect.
    #[cfg(test)]
    pub(super) fn prepare_auto_file_change_action_journal(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
    ) -> Result<PendingActionRecord, String> {
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.prepare_auto_file_change_action_journal_under_deletion_guard(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
            &deletion_lifecycle,
        )
    }

    /// Variant for callers that already hold the deletion lifecycle mutex across the complete
    /// FileChange execution boundary. Passing the guarded state keeps the ownership check explicit
    /// and avoids recursively locking the non-reentrant mutex.
    pub(super) fn prepare_auto_file_change_action_journal_under_deletion_guard(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
        deletion_lifecycle: &DeletionLifecycleState,
    ) -> Result<PendingActionRecord, String> {
        let AgentProposedAction::FileChange { file_change } = &action else {
            return Err("automatic FileChange journal requires a current FileChange action".into());
        };
        if file_change.approval_status != AgentApprovalStatus::Approved
            || file_change.schema_version != mycopilot_core::file_change::FILE_CHANGE_SCHEMA_VERSION
            || file_change.execution.source_tool_name != "apply_patch"
            || file_change.execution.source_call_id != file_change.id
            || file_change.execution.run_id != run_id
            || file_change.execution.conversation_id != conversation_id
            || file_change.execution.validate().is_err()
        {
            return Err("automatic FileChange journal frozen identity is invalid".into());
        }
        let agent_input = bind_pending_provider_configuration(&self.storage, agent_input)?;
        if deletion_lifecycle.contains_input(&agent_input) {
            return Err("项目或会话正在移除，无法启动文件修改。".to_string());
        }
        if !pending_action_binding_matches_for_auto_journal(run_id, None, &action, &agent_input) {
            return Err("automatic FileChange journal checkpoint identity is inconsistent".into());
        }

        let action_id = file_change.id.clone();
        let storage_id = pending_action_storage_id(run_id, &action_id);
        let created_at = now_ms();
        let record = PendingActionRecord {
            storage_id: storage_id.clone(),
            snapshot: PendingAgentActionSnapshot {
                action_id: action_id.clone(),
                run_id: run_id.to_string(),
                conversation_id: Some(conversation_id.to_string()),
                assistant_message_id: Some(assistant_message_id.to_string()),
                action_type: "file_change".to_string(),
                tool_name: "apply_patch".to_string(),
                tool_call_id: Some(action_id),
                action,
                created_at,
                status: PendingActionStatus::Approved,
            },
            agent_input,
        };
        tool_call_for_pending_record(&record)
            .map_err(|_| "automatic FileChange journal exact ToolCall is invalid".to_string())?;
        let pending = pending_storage_record(&record, created_at)?;
        let audit = auto_action_audit_record(
            run_id,
            Some(conversation_id),
            Some(assistant_message_id),
            &record.agent_input,
            &record.snapshot.action,
            "approved",
            None,
            None,
            None,
            None,
            created_at,
            None,
        );
        let outcome = self
            .storage
            .store_auto_file_change_pending_action_with_audit(pending, audit)?;
        if !matches!(
            outcome,
            PendingActionStoreOutcome::Inserted | PendingActionStoreOutcome::Idempotent
        ) {
            return Err("automatic FileChange journal identity conflict".to_string());
        }
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match pending_actions.entry(storage_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(record.clone());
            }
            std::collections::hash_map::Entry::Occupied(entry)
                if same_pending_action_identity(entry.get(), &record) => {}
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err("automatic FileChange in-memory journal identity conflict".into());
            }
        }
        drop(pending_actions);
        Ok(record)
    }

    /// Atomically claims both durable automatic FileChange journals immediately before commit.
    /// `Ok(true)` is the only result that permits crossing the filesystem effect boundary.
    pub(super) fn claim_auto_file_change_dispatch(
        &self,
        record: &mut PendingActionRecord,
    ) -> Result<bool, String> {
        if record.snapshot.status != PendingActionStatus::Approved {
            return Ok(false);
        }
        let approved_pending = pending_storage_record(record, now_ms())?;
        let approved_audit = auto_action_audit_record(
            &record.snapshot.run_id,
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
            &record.agent_input,
            &record.snapshot.action,
            "approved",
            None,
            None,
            None,
            None,
            record.snapshot.created_at,
            None,
        );
        let executing_agent_input_json = persisted_pending_agent_input_json(
            &record.agent_input,
            PendingActionStatus::Executing,
        )?;
        let expected_run_grant_ref = record
            .agent_input
            .resume_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.file_change_run_grant_ref.as_ref());
        let expected_file_change = match (&record.snapshot.action, expected_run_grant_ref) {
            (AgentProposedAction::FileChange { file_change }, Some(_)) => Some(file_change),
            (_, Some(_)) => {
                return Err(
                    "automatic FileChange grant reference is attached to another action"
                        .to_string(),
                );
            }
            (_, None) => None,
        };
        let expected_run_context = if expected_run_grant_ref.is_some() {
            Some(record.agent_input.context.as_ref().ok_or_else(|| {
                "automatic FileChange grant authority has no frozen Run context".to_string()
            })?)
        } else {
            None
        };
        let claimed = self.storage.claim_auto_file_change_pending_execution(
            &approved_pending,
            &approved_audit,
            expected_run_grant_ref,
            expected_file_change,
            expected_run_context,
            &executing_agent_input_json,
            now_ms(),
        )?;
        if !claimed {
            return Ok(false);
        }
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = pending_actions
            .get_mut(&record.storage_id)
            .ok_or_else(|| "automatic FileChange in-memory journal is missing".to_string())?;
        if !same_pending_action_identity_except_status(current, record)
            || current.snapshot.status != PendingActionStatus::Approved
        {
            return Err(
                "automatic FileChange in-memory journal changed after durable claim".into(),
            );
        }
        current.snapshot.status = PendingActionStatus::Executing;
        record.snapshot.status = PendingActionStatus::Executing;
        Ok(true)
    }

    /// Persists the authoritative automatic FileChange receipt before the ToolResult is returned
    /// to the model loop. A crash between any of these monotonic writes leaves the executing
    /// Pending Action recoverable; no caller may manufacture success from an incomplete receipt.
    pub(super) fn finalize_auto_file_change_action_journal(
        &self,
        record: &mut PendingActionRecord,
        execution: &ActionExecutionDecision,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        if record.snapshot.status != PendingActionStatus::Executing
            || !matches!(
                execution.final_pending_status,
                PendingActionStatus::Completed | PendingActionStatus::Failed
            )
        {
            return Err("automatic FileChange terminal journal state is invalid".to_string());
        }
        let completed_at = now_ms();
        let call = tool_call_for_pending_record(record)
            .map_err(|_| "automatic FileChange exact ToolCall is invalid".to_string())?;
        let mut settled_input = record.agent_input.clone();
        settled_input.approval_decision = Some(AgentApprovalDecision {
            action_id: record.storage_id.clone(),
            status: AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        settled_input.tool_continuation = Some(AgentToolContinuation {
            call,
            result: execution.tool_result.clone(),
        });
        let mut commit_errors = Vec::new();
        let mut committed = false;
        for _ in 0..2 {
            match self.commit_auto_file_change_audited_result_trace_with_continuation(
                record,
                &settled_input,
                execution.final_pending_status,
                completed_at,
                notifications,
            ) {
                Ok(()) => {
                    committed = true;
                    break;
                }
                Err(error) => commit_errors.push(error),
            }
        }
        if !committed {
            return Err(commit_errors.join("; retry: "));
        }
        self.transition_pending_status(record, execution.final_pending_status)?;
        record.snapshot.status = execution.final_pending_status;
        Ok(())
    }

    /// Terminalizes and removes one hidden automatic MCP journal without persisting Tool output.
    pub(super) fn settle_auto_mcp_action_journal(
        &self,
        record: &PendingActionRecord,
        outcome: McpAutoActionJournalTerminalOutcome,
        invocation: Option<&mycopilot_core::AgentMcpToolInvocationEvent>,
    ) -> Result<(), String> {
        let expected_status = pending_status_label(record.snapshot.status);
        let settled = self.storage.settle_auto_mcp_action_journal(
            &record.storage_id,
            expected_status,
            outcome,
            invocation,
            now_ms(),
        )?;
        if !settled {
            return Err("automatic MCP journal terminal CAS was lost".to_string());
        }
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending_actions
            .get(&record.storage_id)
            .is_some_and(|current| same_pending_action_identity_except_status(current, record))
        {
            pending_actions.remove(&record.storage_id);
        }
        Ok(())
    }

    pub(super) fn store_pending_action(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
    ) -> Result<bool, String> {
        self.store_pending_action_internal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn store_pending_action_with_predecessor_settlement(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
        predecessor: &PendingActionRecord,
        predecessor_terminal_status: PendingActionStatus,
    ) -> Result<bool, String> {
        self.store_pending_action_internal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
            Some((predecessor, predecessor_terminal_status)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn store_pending_action_internal(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
        predecessor_settlement: Option<(&PendingActionRecord, PendingActionStatus)>,
    ) -> Result<bool, String> {
        let agent_input = bind_pending_provider_configuration(&self.storage, agent_input)?;
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deletion_lifecycle.contains_input(&agent_input) {
            return Err("项目或会话正在移除，无法发布待审批操作。".to_string());
        }
        if !pending_action_binding_matches(run_id, None, &action, &agent_input) {
            self.invalidate_mcp_pending_payload(&action);
            return Err("Pending action frozen Tool Call identity is inconsistent.".to_string());
        }
        let action_id = action_id_for_action(&action);
        let tool_call_id = match &action {
            AgentProposedAction::McpToolCall { approval } => approval.identity.call_id.clone(),
            AgentProposedAction::BuiltinCapabilityActivation { approval } => {
                approval.call_id.clone()
            }
            AgentProposedAction::BuiltinMcpToolApproval { approval } => {
                approval.identity.call_id.clone()
            }
            _ => action_id.clone(),
        };
        let storage_id = pending_action_storage_id(run_id, &action_id);
        let pending_record = PendingActionRecord {
            storage_id: storage_id.clone(),
            snapshot: PendingAgentActionSnapshot {
                action_id: action_id.clone(),
                run_id: run_id.to_string(),
                conversation_id: normalized_optional(Some(conversation_id)),
                assistant_message_id: normalized_optional(Some(assistant_message_id)),
                action_type: action_type_for_action(&action).to_string(),
                tool_name: tool_name_for_action(&action),
                tool_call_id: Some(tool_call_id),
                action,
                created_at: now_ms(),
                status: PendingActionStatus::Pending,
            },
            agent_input,
        };
        tool_call_for_pending_record(&pending_record)
            .map_err(|_| "Pending action frozen Tool Call identity is inconsistent.".to_string())?;
        let approval_notification = self.human_root_approval_notification(
            run_id,
            conversation_id,
            assistant_message_id,
            &action_id,
            pending_record.snapshot.created_at,
        )?;

        if let Some((predecessor, terminal_status)) = predecessor_settlement {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(existing) = pending_actions.get(&storage_id) {
                if !same_pending_action_identity(existing, &pending_record) {
                    return Err(format!(
                        "待审批操作 actionId={action_id} 与内存中的冻结快照冲突（existingRunId={}，candidateRunId={}）。",
                        existing.snapshot.run_id, pending_record.snapshot.run_id
                    ));
                }
            }
            let predecessor_current =
                pending_actions
                    .get(&predecessor.storage_id)
                    .ok_or_else(|| {
                        format!("前置待审批操作的内存状态不存在：{}", predecessor.storage_id)
                    })?;
            if !same_pending_action_identity_except_status(predecessor_current, predecessor) {
                return Err("前置待审批操作的冻结身份已经变化。".to_string());
            }
            let predecessor_expected_status = predecessor_current.snapshot.status;
            let predecessor_terminal_agent_input_json = persisted_pending_agent_input_json(
                &predecessor_current.agent_input,
                terminal_status,
            )?;
            let successor_storage_record = pending_storage_record(&pending_record, now_ms())?;
            let builtin_initial_audit = matches!(
                &pending_record.snapshot.action,
                AgentProposedAction::BuiltinCapabilityActivation { .. }
            )
            .then(|| {
                action_audit_record(
                    &pending_record,
                    None,
                    "pending",
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            });
            let updated_at = now_ms();
            let storage_result = match (builtin_initial_audit, approval_notification.as_ref()) {
                (Some(audit), Some(notification)) => self
                    .storage
                    .store_builtin_capability_pending_action_with_predecessor_settlement_audit_and_notification(
                        successor_storage_record,
                        audit,
                        notification,
                        &predecessor.storage_id,
                        &predecessor.snapshot.action_id,
                        pending_status_label(predecessor_expected_status),
                        pending_status_label(terminal_status),
                        &predecessor_terminal_agent_input_json,
                        updated_at,
                    ),
                (Some(audit), None) => self.storage
                    .store_builtin_capability_pending_action_with_predecessor_settlement_and_audit(
                        successor_storage_record,
                        audit,
                        &predecessor.storage_id,
                        pending_status_label(predecessor_expected_status),
                        pending_status_label(terminal_status),
                        &predecessor_terminal_agent_input_json,
                        updated_at,
                    ),
                (None, Some(notification)) => self.storage
                    .store_pending_agent_action_with_predecessor_settlement_and_notification(
                        successor_storage_record,
                        notification,
                        &predecessor.storage_id,
                        &predecessor.snapshot.action_id,
                        pending_status_label(predecessor_expected_status),
                        pending_status_label(terminal_status),
                        &predecessor_terminal_agent_input_json,
                        updated_at,
                    ),
                (None, None) => self.storage
                    .store_pending_agent_action_with_predecessor_settlement(
                        successor_storage_record,
                        &predecessor.storage_id,
                        pending_status_label(predecessor_expected_status),
                        pending_status_label(terminal_status),
                        &predecessor_terminal_agent_input_json,
                        updated_at,
                    ),
            };
            let storage_outcome = match storage_result {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.invalidate_mcp_pending_payload(&pending_record.snapshot.action);
                    return Err(error);
                }
            };

            let should_publish = match pending_actions.get(&storage_id) {
                Some(existing) if same_pending_action_identity(existing, &pending_record) => false,
                Some(existing) => {
                    self.invalidate_mcp_pending_payload(&pending_record.snapshot.action);
                    return Err(format!(
                        "待审批操作 actionId={action_id} 与内存中的冻结快照冲突（existingRunId={}，candidateRunId={}）。",
                        existing.snapshot.run_id, pending_record.snapshot.run_id
                    ));
                }
                None => {
                    pending_actions.insert(storage_id, pending_record.clone());
                    true
                }
            };
            pending_actions
                .get_mut(&predecessor.storage_id)
                .expect("predecessor was validated under the same pending-action lock")
                .snapshot
                .status = terminal_status;
            drop(pending_actions);
            if matches!(storage_outcome, PendingActionStoreOutcome::Inserted)
                && !matches!(
                    &pending_record.snapshot.action,
                    AgentProposedAction::BuiltinCapabilityActivation { .. }
                )
            {
                self.record_action_audit(
                    &pending_record,
                    None,
                    "pending",
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                );
            }
            drop(deletion_lifecycle);
            return Ok(should_publish);
        }

        {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(existing) = pending_actions.get(&storage_id) {
                if !same_pending_action_identity(existing, &pending_record) {
                    return Err(format!(
                        "待审批操作 actionId={action_id} 与内存中的冻结快照冲突（existingRunId={}，candidateRunId={}）。",
                        existing.snapshot.run_id, pending_record.snapshot.run_id
                    ));
                }
            }
        }
        let storage_result = if matches!(
            &pending_record.snapshot.action,
            AgentProposedAction::BuiltinCapabilityActivation { .. }
        ) {
            let pending = pending_storage_record(&pending_record, now_ms())?;
            let audit = action_audit_record(
                &pending_record,
                None,
                "pending",
                None,
                None,
                None,
                None,
                None,
                None,
            );
            if let Some(notification) = approval_notification.as_ref() {
                self.storage
                    .store_builtin_capability_pending_action_with_audit_and_notification(
                        pending,
                        audit,
                        notification,
                    )
            } else {
                self.storage
                    .store_builtin_capability_pending_action_with_audit(pending, audit)
            }
        } else {
            let pending = pending_storage_record(&pending_record, now_ms())?;
            if let Some(notification) = approval_notification.as_ref() {
                self.storage
                    .store_pending_agent_action_with_notification(pending, notification)
            } else {
                self.storage.store_pending_agent_action(pending)
            }
        };
        let storage_outcome = match storage_result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.invalidate_mcp_pending_payload(&pending_record.snapshot.action);
                return Err(error);
            }
        };
        let should_publish = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match pending_actions.get(&storage_id) {
                Some(existing) if same_pending_action_identity(existing, &pending_record) => false,
                Some(existing) => {
                    self.invalidate_mcp_pending_payload(&pending_record.snapshot.action);
                    return Err(format!(
                        "待审批操作 actionId={action_id} 与内存中的冻结快照冲突（existingRunId={}，candidateRunId={}）。",
                        existing.snapshot.run_id, pending_record.snapshot.run_id
                    ));
                }
                None => {
                    pending_actions.insert(storage_id, pending_record.clone());
                    true
                }
            }
        };
        if matches!(storage_outcome, PendingActionStoreOutcome::Inserted)
            && !matches!(
                &pending_record.snapshot.action,
                AgentProposedAction::BuiltinCapabilityActivation { .. }
            )
        {
            self.record_action_audit(
                &pending_record,
                None,
                "pending",
                None,
                None,
                None,
                None,
                None,
                None,
            );
        }
        drop(deletion_lifecycle);
        Ok(should_publish)
    }
}
