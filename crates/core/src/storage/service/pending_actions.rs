use super::*;

pub(super) fn is_valid_pending_successor(
    interrupted: &AgentPendingActionRecord,
    candidate: &AgentPendingActionRecord,
) -> bool {
    if !is_pending_successor_candidate(interrupted, candidate) {
        return false;
    }
    let Ok(action) = serde_json::from_str::<AgentProposedAction>(&candidate.action_json) else {
        return false;
    };
    let action_id = match &action {
        AgentProposedAction::ToolCall { call } => call.id.as_str(),
        AgentProposedAction::Diff { diff } => diff.id.as_str(),
        AgentProposedAction::FileWrite { file_write } => file_write.id.as_str(),
        AgentProposedAction::Command { command } => command.id.as_str(),
    };
    if candidate.tool_call_id.as_deref() != Some(action_id) {
        return false;
    }
    let Ok(input) = serde_json::from_str::<AgentChatInput>(&candidate.agent_input_json) else {
        return false;
    };
    let Some(checkpoint) = input.resume_checkpoint.as_ref() else {
        return false;
    };
    if checkpoint.run_id != candidate.run_id || checkpoint.pending_tool_call_id != action_id {
        return false;
    }
    let Some(parent_call_id) = interrupted.tool_call_id.as_deref() else {
        return false;
    };
    let parent_result_sequence =
        checkpoint
            .conversation_trace_items
            .iter()
            .find_map(|item| match item {
                ConversationTurnTraceItem::ToolResult {
                    sequence, call_id, ..
                } if call_id == parent_call_id => Some(*sequence),
                _ => None,
            });
    let child_call_sequence =
        checkpoint
            .conversation_trace_items
            .iter()
            .find_map(|item| match item {
                ConversationTurnTraceItem::ToolCall {
                    sequence, call_id, ..
                } if call_id == action_id => Some(*sequence),
                _ => None,
            });
    matches!(
        (parent_result_sequence, child_call_sequence),
        (Some(parent), Some(child)) if parent < child
    )
}

pub(super) fn is_pending_successor_candidate(
    interrupted: &AgentPendingActionRecord,
    candidate: &AgentPendingActionRecord,
) -> bool {
    candidate.status == "pending"
        && candidate.action_id != interrupted.action_id
        && candidate.run_id == interrupted.run_id
        && candidate.conversation_id == interrupted.conversation_id
        && candidate.assistant_message_id == interrupted.assistant_message_id
}

impl StorageService {
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

    pub fn list_pending_agent_actions(&self) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_pending_actions(&connection).map_err(storage_error)
    }

    pub fn reconcile_interrupted_pending_agent_actions(
        &self,
        updated_at: i64,
    ) -> Result<Vec<AgentPendingActionRecord>, String> {
        const INTERRUPTION_REASON: &str =
            "The application exited after approval; command outcome is unknown and was not replayed.";
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let interrupted = pending_action_repository::list_interrupted_actions(&transaction)
            .map_err(storage_error)?;
        let pending_successors =
            pending_action_repository::list_pending_actions(&transaction).map_err(storage_error)?;
        let mut retired_successors = HashSet::new();
        let mut claimed_successors = HashSet::new();
        for record in &interrupted {
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
                            "启动对账无法为旧待审批操作 {} 写入 failed 目标终态。",
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
            let successor_candidates = pending_successors
                .iter()
                .filter(|candidate| {
                    !retired_successors.contains(&candidate.action_id)
                        && is_pending_successor_candidate(record, candidate)
                })
                .collect::<Vec<_>>();
            let valid_successors = successor_candidates
                .iter()
                .copied()
                .filter(|candidate| is_valid_pending_successor(record, candidate))
                .collect::<Vec<_>>();
            if valid_successors.len() > 1 {
                return Err(format!(
                    "启动对账发现 action {} 存在多个合法待审批后继。",
                    record.action_id
                ));
            }
            for candidate in successor_candidates
                .iter()
                .copied()
                .filter(|candidate| !is_valid_pending_successor(record, candidate))
            {
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
                transaction
                    .execute(
                        "UPDATE agent_usage_records
                         SET status = ?2, completed_at = COALESCE(completed_at, ?3)
                         WHERE run_id = ?1
                           AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')",
                        rusqlite::params![record.run_id, durable_run_status, updated_at],
                    )
                    .map_err(storage_error)?;
                continue;
            }
            if let (Some(conversation_id), Some(message_id)) = (
                record.conversation_id.as_deref(),
                record.assistant_message_id.as_deref(),
            ) {
                chat_repository::update_message_run_terminal_state(
                    &transaction,
                    conversation_id,
                    message_id,
                    Some("error"),
                    "failed",
                    updated_at,
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
        }
        transaction.commit().map_err(storage_error)?;
        Ok(interrupted)
    }

    pub fn transition_pending_agent_action(
        &self,
        action_id: &str,
        expected_status: &str,
        status: &str,
        agent_input_json: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        let affected = pending_action_repository::transition_pending_action(
            &connection,
            action_id,
            expected_status,
            status,
            agent_input_json,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作状态迁移必须且只能更新一条记录，actionId={action_id}，实际更新 {affected} 条。"
            ));
        }
        Ok(())
    }

    pub fn set_pending_agent_action_target_status(
        &self,
        action_id: &str,
        expected_status: &str,
        target_status: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        let affected = pending_action_repository::set_pending_action_target_status(
            &connection,
            action_id,
            expected_status,
            target_status,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作目标终态写入必须且只能更新一条记录，actionId={action_id}，expectedStatus={expected_status}，实际更新 {affected} 条。"
            ));
        }
        Ok(())
    }
}
