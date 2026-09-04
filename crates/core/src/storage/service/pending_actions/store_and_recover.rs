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
            let command_process_is_known_stopped = match pending.as_ref() {
                Some(pending) => {
                    manual_command_has_resolved_terminal_session(&connection, pending)?
                }
                None => false,
            };
            if !is_authoritatively_settled && !command_process_is_known_stopped {
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

    pub fn get_agent_action_audit(
        &self,
        action_id: &str,
    ) -> Result<Option<AgentActionAuditRecord>, String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::load_action_audit_record(&connection, action_id)
            .map_err(storage_error)
    }

    /// Returns current canonical executing FileChange audit records for startup reconciliation.
    /// Payload validation remains the recovery caller's responsibility.
    pub fn list_executing_file_change_action_audits(
        &self,
    ) -> Result<Vec<AgentActionAuditRecord>, String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::list_executing_file_change_action_audits(&connection)
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
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let tree_stopped = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_tree_run_stops WHERE run_id = ?1
                 )",
                [&record.run_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_error)?;
        if tree_stopped {
            return Err("stopped Agent Run cannot claim automatic action execution".to_string());
        }
        let outcome =
            agent_action_audit_repository::claim_action_audit_execution(&transaction, &record)
                .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    pub fn finalize_agent_action_audit_execution(
        &self,
        record: AgentActionAuditRecord,
    ) -> Result<agent_action_audit_repository::AgentActionAuditFinalizationOutcome, String> {
        if !matches!(record.status.as_str(), "completed" | "failed" | "cancelled") {
            return Err(format!(
                "自动操作审计终态无效：{}（仅允许 completed、failed 或 cancelled）。",
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

    /// Persists the committed Direct FileChange credential for a manually approved action.
    ///
    /// Only the private `action_json` and its monotonic update timestamp change. The pending row
    /// must retain the exact frozen owner/Tool identity and remain `executing`.
    pub fn commit_pending_direct_file_change_action_json(
        &self,
        identity: &AgentPendingActionRecord,
        expected_action_json: &str,
        committed_action_json: &str,
        updated_at: i64,
    ) -> Result<AgentPendingActionJsonCommitOutcome, String> {
        if !is_current_file_change_action_identity(&identity.action_type, &identity.tool_name)
            || identity.tool_call_id.as_deref().is_none_or(str::is_empty)
            || identity.status != "executing"
            || identity.target_status.is_some()
            || updated_at < identity.updated_at
        {
            return Err("manual Direct FileChange pending identity is invalid".to_string());
        }
        validate_file_change_action_json_pair(
            &identity.action_json,
            expected_action_json,
            committed_action_json,
            CurrentFileChangeActionIdentity {
                run_id: &identity.run_id,
                conversation_id: identity.conversation_id.as_deref(),
                tool_call_id: identity.tool_call_id.as_deref(),
                action_type: &identity.action_type,
                tool_name: &identity.tool_name,
            },
        )?;
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = pending_action_repository::commit_executing_action_json(
            &transaction,
            identity,
            expected_action_json,
            committed_action_json,
            updated_at,
        )
        .map_err(storage_error)?;
        if matches!(
            outcome,
            AgentPendingActionJsonCommitOutcome::Updated
                | AgentPendingActionJsonCommitOutcome::AlreadyCommitted
        ) {
            let audit_outcome =
                agent_action_audit_repository::commit_pending_file_change_action_json_if_present(
                    &transaction,
                    identity,
                    expected_action_json,
                    committed_action_json,
                )
                .map_err(storage_error)?;
            match audit_outcome {
                agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::Updated
                | agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::AlreadyCommitted
                | agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::Absent => {}
                agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::ExpectedActionMismatch => {
                    return Err("manual Direct FileChange audit action JSON changed before commit"
                        .to_string());
                }
                agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::NotPreterminal {
                    status,
                } => {
                    return Err(format!(
                        "manual Direct FileChange audit is no longer preterminal: status={status}"
                    ));
                }
                agent_action_audit_repository::ManualActionAuditJsonCommitOutcome::IdentityConflict {
                    status,
                } => {
                    return Err(format!(
                        "manual Direct FileChange audit identity conflict: status={status}"
                    ));
                }
            }
        }
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Publishes an actionable approval and its user notification as one visibility boundary.
    pub fn store_pending_agent_action_with_notification(
        &self,
        record: AgentPendingActionRecord,
        notification: &notification_repository::NewNotificationEventRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = store_pending_action_or_conflict(&transaction, &record)?;
        notification_repository::enqueue_notification_event_in_transaction(
            &transaction,
            notification,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Atomically publishes one built-in capability approval and its initial manual audit.
    ///
    /// A recoverable approval without its audit row is an externally observable split-brain:
    /// the approval UI can survive restart while the durable action journal omits the proposal.
    /// Keeping both inserts in one immediate transaction also makes an exact replay idempotent and
    /// rejects an action id already owned by a different audit identity without leaving an orphan
    /// pending row.
    pub fn store_builtin_capability_pending_action_with_audit(
        &self,
        pending: AgentPendingActionRecord,
        audit: AgentActionAuditRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_builtin_capability_initial_audit(&pending, &audit)?;
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = store_pending_action_or_conflict(&transaction, &pending)?;
        ensure_exact_builtin_capability_initial_audit(&transaction, &audit)?;
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Atomically persists the hidden auto-approval journal and its frozen audit before a
    /// FileChange may cross the filesystem effect boundary.
    ///
    /// The pending row owns the exact resumable ToolCall/checkpoint envelope. The audit owns the
    /// one allowed dispatch claim. Neither record is useful alone, so publishing both in one
    /// immediate transaction prevents a crash from leaving an executable but unrecoverable
    /// action.
    pub fn store_auto_file_change_pending_action_with_audit(
        &self,
        pending: AgentPendingActionRecord,
        audit: AgentActionAuditRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_auto_file_change_initial_journal(&pending, &audit)?;
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = store_pending_action_or_conflict(&transaction, &pending)?;
        ensure_exact_auto_file_change_initial_audit(&transaction, &audit)?;
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Atomically crosses the current automatic FileChange pre-dispatch boundary.
    ///
    /// Both the hidden Pending Action and its exact automatic approval audit must still be the
    /// byte-for-byte approved journal written by
    /// [`Self::store_auto_file_change_pending_action_with_audit`]. The filesystem committer may
    /// run only after this transaction returns `true`.
    #[allow(clippy::too_many_arguments)]
    pub fn claim_auto_file_change_pending_execution(
        &self,
        pending: &AgentPendingActionRecord,
        approved_audit: &AgentActionAuditRecord,
        expected_run_grant_ref: Option<&crate::file_change::FileChangeRunGrantRef>,
        expected_file_change: Option<&crate::AgentFileChangeProposal>,
        expected_run_context: Option<&crate::AgentRunContext>,
        executing_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<bool, String> {
        validate_auto_file_change_initial_journal(pending, approved_audit)?;
        let uses_run_grant = approved_audit.decision_source.as_deref() == Some("run_grant");
        if uses_run_grant
            != (expected_run_grant_ref.is_some()
                && expected_file_change.is_some()
                && expected_run_context.is_some())
            || (!uses_run_grant
                && (expected_file_change.is_some() || expected_run_context.is_some()))
        {
            return Err("automatic FileChange grant authority is inconsistent".to_string());
        }
        if let Some(grant_ref) = expected_run_grant_ref {
            grant_ref
                .validate()
                .map_err(|_| "automatic FileChange grant reference is invalid".to_string())?;
        }
        if updated_at < pending.updated_at || executing_agent_input_json.trim().is_empty() {
            return Err("automatic FileChange executing journal is invalid".to_string());
        }
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let Some(existing_pending) =
            pending_action_repository::load_pending_action(&transaction, &pending.action_id)
                .map_err(storage_error)?
        else {
            transaction.commit().map_err(storage_error)?;
            return Ok(false);
        };
        let pending_matches = existing_pending.action_id == pending.action_id
            && existing_pending.run_id == pending.run_id
            && existing_pending.conversation_id == pending.conversation_id
            && existing_pending.assistant_message_id == pending.assistant_message_id
            && existing_pending.action_type == pending.action_type
            && existing_pending.tool_name == pending.tool_name
            && existing_pending.tool_call_id == pending.tool_call_id
            && existing_pending.status == "approved"
            && existing_pending.target_status.is_none()
            && existing_pending.action_json == pending.action_json
            && existing_pending.agent_input_json == pending.agent_input_json
            && existing_pending.created_at == pending.created_at;
        let existing_audit = agent_action_audit_repository::load_action_audit_record(
            &transaction,
            &approved_audit.action_id,
        )
        .map_err(storage_error)?;
        let audit_matches = existing_audit.as_ref().is_some_and(|existing| {
            same_builtin_capability_initial_audit(existing, approved_audit)
        });
        if !pending_matches || !audit_matches {
            transaction.commit().map_err(storage_error)?;
            return Ok(false);
        }
        if let (Some(grant_ref), Some(file_change), Some(run_context)) = (
            expected_run_grant_ref,
            expected_file_change,
            expected_run_context,
        ) {
            let grant = file_change_run_grant_repository::get_active_run_grant(
                &transaction,
                &pending.run_id,
            )
            .map_err(storage_error)?;
            let Some(grant) = grant else {
                transaction.commit().map_err(storage_error)?;
                return Ok(false);
            };
            if !super::file_change_run_grants::file_change_run_grant_authorizes(
                &grant,
                Some(grant_ref),
                file_change,
                run_context,
            )
            .map_err(|error| error.to_string())?
            {
                transaction.commit().map_err(storage_error)?;
                return Ok(false);
            }
        }
        let pending_changed = pending_action_repository::transition_pending_action_for_dispatch(
            &transaction,
            &pending.action_id,
            "approved",
            "executing",
            executing_agent_input_json,
            updated_at,
        )
        .map_err(storage_error)?;
        let audit_changed = transaction
            .execute(
                "
                UPDATE agent_action_audit
                SET status = 'executing'
                WHERE action_id = ?1
                  AND status = 'approved'
                  AND decision = 'approved'
                  AND decision_source = ?11
                  AND run_id = ?2
                  AND conversation_id IS ?3
                  AND assistant_message_id IS ?4
                  AND action_type = 'file_change'
                  AND tool_name = 'apply_patch'
                  AND action_json = ?5
                  AND created_at = ?6
                  AND decided_at IS ?7
                  AND effective_permissions_json IS ?8
                  AND path_scope IS ?9
                  AND command_cwd_scope IS ?10
                  AND file_change_result_json IS NULL
                  AND command_result_json IS NULL
                  AND tool_result_json IS NULL
                  AND error IS NULL
                  AND completed_at IS NULL
                  AND blocked_reason IS NULL
                ",
                rusqlite::params![
                    approved_audit.action_id,
                    approved_audit.run_id,
                    approved_audit.conversation_id,
                    approved_audit.assistant_message_id,
                    approved_audit.action_json,
                    approved_audit.created_at,
                    approved_audit.decided_at,
                    approved_audit.effective_permissions_json,
                    approved_audit.path_scope,
                    approved_audit.command_cwd_scope,
                    approved_audit.decision_source,
                ],
            )
            .map_err(storage_error)?;
        if pending_changed != 1 || audit_changed != 1 {
            return Err("automatic FileChange dispatch journal lost its exact CAS".to_string());
        }
        transaction.commit().map_err(storage_error)?;
        Ok(true)
    }

    pub fn store_builtin_capability_pending_action_with_audit_and_notification(
        &self,
        pending: AgentPendingActionRecord,
        audit: AgentActionAuditRecord,
        notification: &notification_repository::NewNotificationEventRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_builtin_capability_initial_audit(&pending, &audit)?;
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = store_pending_action_or_conflict(&transaction, &pending)?;
        ensure_exact_builtin_capability_initial_audit(&transaction, &audit)?;
        notification_repository::enqueue_notification_event_in_transaction(
            &transaction,
            notification,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Publishes a successor approval and terminalizes its predecessor as one durable fact.
    ///
    /// The predecessor's paired ToolResult and `target_status` were committed before model
    /// continuation. Once that continuation proposes another approval, the successor row becomes
    /// the recovery anchor for the Run. The insert and predecessor CAS must therefore commit
    /// together: exposing either half alone can leave an actionable orphan approval or lose the
    /// only continuation checkpoint.
    #[allow(clippy::too_many_arguments)]
    pub fn store_pending_agent_action_with_predecessor_settlement(
        &self,
        successor: AgentPendingActionRecord,
        predecessor_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        self.store_pending_agent_action_with_predecessor_settlement_internal(
            successor,
            predecessor_action_id,
            predecessor_expected_status,
            predecessor_terminal_status,
            predecessor_terminal_agent_input_json,
            updated_at,
            None,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn store_pending_agent_action_with_predecessor_settlement_and_notification(
        &self,
        successor: AgentPendingActionRecord,
        successor_notification: &notification_repository::NewNotificationEventRecord,
        predecessor_action_id: &str,
        predecessor_renderer_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        self.store_pending_agent_action_with_predecessor_settlement_internal(
            successor,
            predecessor_action_id,
            predecessor_expected_status,
            predecessor_terminal_status,
            predecessor_terminal_agent_input_json,
            updated_at,
            None,
            Some(successor_notification),
            Some(predecessor_renderer_action_id),
        )
    }

    /// Built-in capability variant of predecessor settlement. The successor pending row and its
    /// initial audit share the same transaction as the predecessor terminal CAS.
    #[allow(clippy::too_many_arguments)]
    pub fn store_builtin_capability_pending_action_with_predecessor_settlement_and_audit(
        &self,
        successor: AgentPendingActionRecord,
        successor_audit: AgentActionAuditRecord,
        predecessor_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_builtin_capability_initial_audit(&successor, &successor_audit)?;
        self.store_pending_agent_action_with_predecessor_settlement_internal(
            successor,
            predecessor_action_id,
            predecessor_expected_status,
            predecessor_terminal_status,
            predecessor_terminal_agent_input_json,
            updated_at,
            Some(successor_audit),
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn store_builtin_capability_pending_action_with_predecessor_settlement_audit_and_notification(
        &self,
        successor: AgentPendingActionRecord,
        successor_audit: AgentActionAuditRecord,
        successor_notification: &notification_repository::NewNotificationEventRecord,
        predecessor_action_id: &str,
        predecessor_renderer_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        validate_builtin_capability_initial_audit(&successor, &successor_audit)?;
        self.store_pending_agent_action_with_predecessor_settlement_internal(
            successor,
            predecessor_action_id,
            predecessor_expected_status,
            predecessor_terminal_status,
            predecessor_terminal_agent_input_json,
            updated_at,
            Some(successor_audit),
            Some(successor_notification),
            Some(predecessor_renderer_action_id),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn store_pending_agent_action_with_predecessor_settlement_internal(
        &self,
        successor: AgentPendingActionRecord,
        predecessor_action_id: &str,
        predecessor_expected_status: &str,
        predecessor_terminal_status: &str,
        predecessor_terminal_agent_input_json: &str,
        updated_at: i64,
        successor_audit: Option<AgentActionAuditRecord>,
        successor_notification: Option<&notification_repository::NewNotificationEventRecord>,
        predecessor_renderer_action_id: Option<&str>,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        if successor.action_id == predecessor_action_id {
            return Err("successor approval must not replace its predecessor".to_string());
        }
        if successor.status != "pending" || successor.target_status.is_some() {
            return Err("successor approval must be a fresh pending action".to_string());
        }
        if !matches!(
            predecessor_terminal_status,
            "rejected" | "cancelled" | "completed" | "failed"
        ) {
            return Err("predecessor settlement requires a terminal target status".to_string());
        }

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let predecessor =
            pending_action_repository::load_pending_action(&transaction, predecessor_action_id)
                .map_err(storage_error)?
                .ok_or_else(|| "successor approval has no durable predecessor".to_string())?;
        if predecessor.run_id != successor.run_id
            || predecessor.conversation_id != successor.conversation_id
            || predecessor.assistant_message_id != successor.assistant_message_id
        {
            return Err("successor approval and predecessor ownership do not match".to_string());
        }
        if predecessor.target_status.as_deref() != Some(predecessor_terminal_status) {
            return Err(
                "predecessor terminal target was not durably committed before its successor"
                    .to_string(),
            );
        }
        let predecessor_already_terminal = predecessor.status == predecessor_terminal_status;
        if !predecessor_already_terminal && predecessor.status != predecessor_expected_status {
            return Err(format!(
                "predecessor status changed before successor publication: expected={predecessor_expected_status}, actual={}",
                predecessor.status
            ));
        }

        let outcome = store_pending_action_or_conflict(&transaction, &successor)?;

        if let Some(audit) = successor_audit.as_ref() {
            ensure_exact_builtin_capability_initial_audit(&transaction, audit)?;
        }

        if !predecessor_already_terminal {
            let affected = pending_action_repository::transition_pending_action(
                &transaction,
                predecessor_action_id,
                predecessor_expected_status,
                predecessor_terminal_status,
                predecessor_terminal_agent_input_json,
                updated_at,
            )
            .map_err(storage_error)?;
            if affected != 1 {
                return Err(format!(
                    "前置待审批操作终态迁移必须且只能更新一条记录，actionId={predecessor_action_id}，实际更新 {affected} 条。"
                ));
            }
        }
        if let Some(predecessor_renderer_action_id) = predecessor_renderer_action_id {
            notification_repository::resolve_notification_events_by_run_and_approval_action_id_in_transaction(
                &transaction,
                &successor.run_id,
                predecessor_renderer_action_id,
                updated_at,
            )
            .map_err(storage_error)?;
        }
        if let Some(notification) = successor_notification {
            notification_repository::enqueue_notification_event_in_transaction(
                &transaction,
                notification,
            )
            .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    pub fn list_pending_agent_actions(&self) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_pending_actions(&connection).map_err(storage_error)
    }

    /// Lists crash-interrupted rows for the Host's typed pre-reconciliation pass.
    ///
    /// Callers may inspect current-format private action receipts before the generic startup
    /// terminalizer runs. This is read-only and does not make an interrupted action replayable.
    pub fn list_interrupted_agent_actions_for_host_reconciliation(
        &self,
    ) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_interrupted_actions(&connection).map_err(storage_error)
    }

    /// Loads one frozen approval fact by its backend-framed id. Renderer-facing callers must use
    /// a higher-level root projection and must never receive `agent_input_json`.
    pub fn get_pending_agent_action(
        &self,
        approval_id: &str,
    ) -> Result<Option<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::load_pending_action(&connection, approval_id)
            .map_err(storage_error)
    }

    /// Returns the current-format rows owned by pending or MCP-specific startup recovery.
    pub fn list_recoverable_agent_actions_after_reconciliation(
        &self,
    ) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_recoverable_actions_after_reconciliation(&connection)
            .map_err(storage_error)
    }

    /// Returns whether a nonterminal action in the same durable Run must settle before the
    /// successor approval can resume model execution.
    ///
    /// The successor's frozen checkpoint selects candidate ToolResult ids, while the Conversation
    /// trace proves that a selected result is already durable before the successor ToolCall. This
    /// deliberately does not infer dependencies from creation time or sibling ToolCalls. Reading
    /// current SQLite status also avoids a false block when a terminal CAS committed but its
    /// caller observed an unknown response and process-local state remained stale.
    pub fn pending_agent_action_has_unsettled_predecessor(
        &self,
        successor_action_id: &str,
        frozen_result_call_ids: &[String],
    ) -> Result<bool, String> {
        if frozen_result_call_ids.is_empty() {
            return Ok(false);
        }
        let connection = self.state.connection()?;
        let Some(successor) =
            pending_action_repository::load_pending_action(&connection, successor_action_id)
                .map_err(storage_error)?
        else {
            // Preserve the approval path's existing durable status CAS as the authoritative
            // fail-closed boundary for a concurrently removed successor row. The predecessor
            // gate only adds dependency ordering; it must not replace missing-row arbitration.
            return Ok(false);
        };
        let unsettled =
            pending_action_repository::list_recoverable_actions_after_reconciliation(&connection)
                .map_err(storage_error)?;
        for predecessor in unsettled {
            if predecessor.tool_call_id.as_ref().is_some_and(|call_id| {
                frozen_result_call_ids
                    .iter()
                    .any(|frozen| frozen == call_id)
            }) && durable_trace_proves_action_precedes(&predecessor, &successor, &connection)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn reconcile_interrupted_pending_agent_actions(
        &self,
        updated_at: i64,
    ) -> Result<Vec<AgentPendingActionRecord>, String> {
        const INTERRUPTION_REASON: &str =
            "The application exited after approval; process outcome is unknown and was not replayed.";
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let interrupted = pending_action_repository::list_interrupted_actions(&transaction)
            .map_err(storage_error)?;
        let pending_successors =
            pending_action_repository::list_pending_actions(&transaction).map_err(storage_error)?;
        let mut retired_successors = HashSet::new();
        let mut claimed_successors = HashSet::new();
        for record in &interrupted {
            if matches!(record.action_type.as_str(), "mcp_tool_call" | "file_change")
                && record.status == "executing"
                && !manual_file_effect_has_authoritative_settlement(&transaction, record)?
            {
                return Err(format!(
                    "启动对账拒绝采用缺少权威审计与 ToolResult 证据的文件副作用目标终态：{}",
                    record.action_id
                ));
            }
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
            let reconciled_status = match record.target_status.as_deref() {
                Some(status @ ("completed" | "failed" | "rejected" | "cancelled")) => status,
                None => {
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
                            "启动对账无法为中断的待审批操作 {} 写入 failed 目标终态。",
                            record.action_id
                        ));
                    }
                    "failed"
                }
                Some(status) => {
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
            resolve_pending_approval_notification_in_transaction(&transaction, record, updated_at)?;
            let successor_candidates = pending_successors
                .iter()
                .filter(|candidate| {
                    !retired_successors.contains(&candidate.action_id)
                        && is_pending_successor_candidate(record, candidate)
                })
                .collect::<Vec<_>>();
            let mut valid_successors = Vec::new();
            for candidate in successor_candidates.iter().copied() {
                if is_valid_pending_successor(&transaction, record, candidate)? {
                    valid_successors.push(candidate);
                }
            }
            if valid_successors.len() > 1 {
                return Err(format!(
                    "启动对账发现 action {} 存在多个合法待审批后继。",
                    record.action_id
                ));
            }
            for candidate in successor_candidates.iter().copied().filter(|candidate| {
                !valid_successors
                    .iter()
                    .any(|valid| valid.action_id == candidate.action_id)
            }) {
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
                resolve_pending_approval_notification_in_transaction(
                    &transaction,
                    candidate,
                    updated_at,
                )?;
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
                let assistant_message_id =
                    record.assistant_message_id.as_deref().ok_or_else(|| {
                        format!(
                            "启动对账发现终态 run {} 缺少 Assistant owner。",
                            record.run_id
                        )
                    })?;
                let trace = conversation_trace_repository::get_trace_for_message(
                    &transaction,
                    assistant_message_id,
                )
                .map_err(storage_error)?
                .ok_or_else(|| {
                    format!(
                        "启动对账发现终态 run {} 缺少 ConversationTurnTrace。",
                        record.run_id
                    )
                })?;
                let model_context_items =
                    conversation_model_context_repository::get_log_for_message(
                        &transaction,
                        assistant_message_id,
                    )
                    .map_err(storage_error)?
                    .map(|log| log.items)
                    .unwrap_or_default();
                trace
                    .validate_complete_model_context(&model_context_items)
                    .map_err(|error| {
                        format!(
                            "启动对账发现终态 run {} 的模型历史不完整：{error}",
                            record.run_id
                        )
                    })?;
                transaction
                    .execute(
                        "UPDATE agent_usage_records
                         SET status = ?2, completed_at = COALESCE(completed_at, ?3)
                         WHERE run_id = ?1
                           AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')",
                        rusqlite::params![record.run_id, durable_run_status, updated_at],
                    )
                    .map_err(storage_error)?;
                let notification_kind = match durable_run_status.as_deref() {
                    Some("completed") => "task_completed",
                    Some("failed") => "task_failed",
                    Some("cancelled") => "task_cancelled",
                    _ => unreachable!("terminal status was checked above"),
                };
                let conversation_id = record.conversation_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账发现终态 run {} 缺少 conversation owner。",
                        record.run_id
                    )
                })?;
                super::trace_reconciliation::enqueue_reconciled_human_root_notification(
                    &transaction,
                    &record.run_id,
                    conversation_id,
                    assistant_message_id,
                    notification_kind,
                    updated_at,
                )?;
                continue;
            }
            {
                let conversation_id = record.conversation_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账无法终结 run {}：缺少 conversation owner。",
                        record.run_id
                    )
                })?;
                let message_id = record.assistant_message_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账无法终结 run {}：缺少 Assistant owner。",
                        record.run_id
                    )
                })?;
                let durable = load_durable_pending_trace_snapshot(&transaction, record, false)
                    .map_err(|error| {
                        format!(
                            "启动对账无法终结 run {} 的 durable ToolCall：{error}",
                            record.run_id
                        )
                    })?;
                let terminal = crate::terminal_conversation_trace_from_snapshot(
                    durable.snapshot,
                    &record.run_id,
                    conversation_id,
                    message_id,
                    crate::ConversationTurnTraceTerminalStatus::Failed,
                    INTERRUPTION_REASON,
                )?;
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
                    message_id,
                    &terminal.model_context_items,
                )
                .map_err(storage_error)?;
                chat_repository::update_message_run_terminal_state(
                    &transaction,
                    conversation_id,
                    message_id,
                    &record.run_id,
                    Some("error"),
                    "failed",
                    updated_at,
                    None,
                )
                .map_err(storage_error)?;
            }
            transaction
                .execute(
                    "UPDATE agent_usage_records
                     SET status = 'failed', error = ?2, completed_at = ?3
                     WHERE run_id = ?1
                       AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')",
                    rusqlite::params![record.run_id, INTERRUPTION_REASON, updated_at],
                )
                .map_err(storage_error)?;
            super::trace_reconciliation::enqueue_reconciled_human_root_notification(
                &transaction,
                &record.run_id,
                record.conversation_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账无法通知 run {}：缺少 conversation owner。",
                        record.run_id
                    )
                })?,
                record.assistant_message_id.as_deref().ok_or_else(|| {
                    format!(
                        "启动对账无法通知 run {}：缺少 Assistant owner。",
                        record.run_id
                    )
                })?,
                "task_failed",
                updated_at,
            )?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(interrupted)
    }

}
