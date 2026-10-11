use super::*;
use crate::storage::StorageWriteClass;
#[cfg(test)]
use crate::CreateAgentNodeInput;
use crate::{
    AcknowledgeAgentTaskAndWakeInput, AgentEffectivePermissionSnapshot, AgentGraphError,
    AgentLifecycle, AgentMailboxMessageRecord, AgentNodeRecord, AgentPermissions,
    AgentWakeRequestRecord, AgentWakeStatus, ConversationMessageOrigin, EnqueueAgentMessageInput,
    EnqueueAgentWakeInput, EnsureRootAgentInput, FinishAgentWakeWithResultInput, IdempotentCreate,
};

fn unavailable(error: String) -> AgentGraphError {
    AgentGraphError::StorageUnavailable(error)
}

fn agent_job_bytes(parts: &[&str]) -> usize {
    parts.iter().fold(512_usize, |bytes, part| {
        bytes.saturating_add(part.len().saturating_mul(2))
    })
}
fn wake_job_bytes(input: &EnqueueAgentWakeInput) -> usize {
    agent_job_bytes(&[
        &input.wake_id,
        &input.root_agent_id,
        &input.agent_id,
        &input.requester_agent_id,
        &input.request_id,
        input.source_agent_message_id.as_deref().unwrap_or_default(),
    ])
}
fn message_job_bytes(input: &EnqueueAgentMessageInput) -> usize {
    agent_job_bytes(&[
        &input.message_id,
        &input.root_agent_id,
        &input.sender_agent_id,
        &input.recipient_agent_id,
        &input.request_id,
        &input.content,
        &input.projection_message_id,
    ])
}
fn send_job_bytes(input: &crate::SendAgentMessageRequest) -> usize {
    agent_job_bytes(&[
        &input.sender_agent_id,
        &input.recipient_agent_id,
        &input.request_id,
        &input.content,
    ])
}
fn result_job_bytes(input: &crate::FinishAgentTurnResultInput) -> usize {
    agent_job_bytes(&[
        &input.wake_id,
        &input.claim_token,
        input.run_id.as_deref().unwrap_or_default(),
        input.assistant_message_id.as_deref().unwrap_or_default(),
        input.terminal_error.as_deref().unwrap_or_default(),
    ])
}

impl StorageService {
    pub fn ensure_agent_conversation_deletable(
        &self,
        conversation_id: &str,
    ) -> Result<(), AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[conversation_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::ensure_conversation_unbound(connection, conversation_id)
                },
            )
            .map_err(unavailable)?
    }

    pub fn ensure_agent_project_deletable(&self, project_id: &str) -> Result<(), AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[project_id]),
                StorageWriteClass::Control,
                |connection| agent_graph_repository::ensure_project_unbound(connection, project_id),
            )
            .map_err(unavailable)?
    }

    pub fn ensure_root_agent(
        &self,
        input: &EnsureRootAgentInput,
    ) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[
                    &input.agent_id,
                    &input.conversation_id,
                    &input.creation_request_id,
                    &input.task_name,
                ]),
                StorageWriteClass::Normal,
                |connection| agent_graph_repository::ensure_root_agent(connection, input, now_ms()),
            )
            .map_err(unavailable)?
    }

    #[cfg(test)]
    pub(crate) fn create_agent_node(
        &self,
        input: &CreateAgentNodeInput,
    ) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[
                    &input.agent_id,
                    &input.root_agent_id,
                    &input.parent_agent_id,
                    &input.conversation_id,
                    &input.creation_request_id,
                    &input.task_name,
                    &input.task_path,
                    &input.model_snapshot.model_config_id,
                    &input.model_snapshot.display_name,
                    &input.model_snapshot.model_settings_configuration_revision,
                    &input.model_snapshot.provider_connection_revision,
                    &input.model_snapshot.provider_protocol_revision,
                ])
                .saturating_add(input.template_snapshot.as_ref().map_or(
                    0,
                    |template| {
                        agent_job_bytes(&[
                            &template.template_id,
                            &template.project_id,
                            &template.machine_key,
                            &template.name,
                            &template.description,
                            &template.instructions,
                            &template.model_config_id,
                        ])
                    },
                )),
                StorageWriteClass::Normal,
                |connection| agent_graph_repository::create_agent_node(connection, input, now_ms()),
            )
            .map_err(unavailable)?
    }

    pub fn get_agent_node(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_graph_repository::get_agent_node(&connection, agent_id)
    }

    pub fn get_agent_node_by_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_graph_repository::get_agent_node_by_conversation(&connection, conversation_id)
    }

    pub fn get_agent_effective_permission_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentEffectivePermissionSnapshot>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[agent_id]),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::get_agent_effective_permission_snapshot(
                        connection, agent_id,
                    )
                },
            )
            .map_err(unavailable)?
    }

    /// Records permissions copied from the exact Host-authenticated ToolExecutionContext of an
    /// active Turn. The repository validates Agent/Conversation/Run/assistant identity together.
    #[allow(clippy::too_many_arguments)]
    pub fn record_agent_effective_permissions_for_active_turn(
        &self,
        agent_id: &str,
        conversation_id: &str,
        run_id: &str,
        assistant_message_id: &str,
        permissions: AgentPermissions,
    ) -> Result<AgentEffectivePermissionSnapshot, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[agent_id, conversation_id, run_id, assistant_message_id]),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::record_agent_effective_permissions_for_active_turn(
                        connection,
                        agent_id,
                        conversation_id,
                        run_id,
                        assistant_message_id,
                        permissions,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn list_agent_children(
        &self,
        root_agent_id: &str,
        parent_agent_id: &str,
    ) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_graph_repository::list_agent_children(&connection, root_agent_id, parent_agent_id)
    }

    pub fn list_agent_tree(
        &self,
        root_agent_id: &str,
    ) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_graph_repository::list_agent_tree(&connection, root_agent_id)
    }

    pub fn transition_agent_lifecycle(
        &self,
        agent_id: &str,
        expected_revision: u64,
        expected_lifecycle: AgentLifecycle,
        requested_lifecycle: AgentLifecycle,
    ) -> Result<AgentNodeRecord, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[agent_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::transition_agent_lifecycle(
                        connection,
                        agent_id,
                        expected_revision,
                        expected_lifecycle,
                        requested_lifecycle,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn enqueue_agent_message(
        &self,
        input: &EnqueueAgentMessageInput,
    ) -> Result<IdempotentCreate<AgentMailboxMessageRecord>, AgentGraphError> {
        self.state
            .write_sync(
                message_job_bytes(input),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::enqueue_agent_message(connection, input, now_ms())
                },
            )
            .map_err(unavailable)?
    }

    pub fn get_agent_message(
        &self,
        message_id: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_graph_repository::get_agent_message(&connection, message_id)
    }

    pub fn claim_next_agent_message(
        &self,
        recipient_agent_id: &str,
        claim_token: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[recipient_agent_id, claim_token]),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::claim_next_agent_message(
                        connection,
                        recipient_agent_id,
                        claim_token,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn renew_agent_message_lease(
        &self,
        message_id: &str,
        claim_token: &str,
    ) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[message_id, claim_token]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::renew_agent_message_lease(
                        connection,
                        message_id,
                        claim_token,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn acknowledge_agent_message_with_projection(
        &self,
        message_id: &str,
        claim_token: &str,
    ) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[message_id, claim_token]),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::acknowledge_agent_message_with_projection(
                        connection,
                        message_id,
                        claim_token,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn acknowledge_agent_task_with_projection_and_wake(
        &self,
        input: &AcknowledgeAgentTaskAndWakeInput,
    ) -> Result<(AgentMailboxMessageRecord, AgentWakeRequestRecord), AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[&input.message_id, &input.message_claim_token])
                    .saturating_add(wake_job_bytes(&input.wake)),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::acknowledge_agent_task_with_projection_and_wake(
                        connection,
                        input,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn conversation_message_origin(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<ConversationMessageOrigin, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_graph_repository::conversation_message_origin(
            &connection,
            conversation_id,
            message_id,
        )
    }

    pub fn enqueue_agent_wake(
        &self,
        input: &EnqueueAgentWakeInput,
    ) -> Result<IdempotentCreate<AgentWakeRequestRecord>, AgentGraphError> {
        self.state
            .write_sync(
                wake_job_bytes(input),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::enqueue_agent_wake(connection, input, now_ms())
                },
            )
            .map_err(unavailable)?
    }

    pub fn get_agent_wake(
        &self,
        wake_id: &str,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_graph_repository::get_agent_wake(&connection, wake_id)
    }

    pub fn claim_next_agent_wake(
        &self,
        agent_id: &str,
        claim_token: &str,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[agent_id, claim_token]),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::claim_next_agent_wake(
                        connection,
                        agent_id,
                        claim_token,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn claim_next_dispatchable_agent_wake(
        &self,
        claim_token: &str,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        // Queue wait precedes the claim: the durable lease starts when its transaction runs.
        self.state
            .write_sync(
                agent_job_bytes(&[claim_token]),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::claim_next_dispatchable_agent_wake(
                        connection,
                        claim_token,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn claim_next_dispatchable_agent_wake_at(
        &self,
        claim_token: &str,
        claimed_at: i64,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[claim_token]),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::claim_next_dispatchable_agent_wake(
                        connection,
                        claim_token,
                        claimed_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn recover_agent_wakes(
        &self,
        recovery_token_prefix: &str,
    ) -> Result<crate::AgentWakeRecoveryBatch, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[recovery_token_prefix]),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::recover_agent_wakes(
                        connection,
                        recovery_token_prefix,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn recover_agent_wakes_at(
        &self,
        recovery_token_prefix: &str,
        recovered_at: i64,
    ) -> Result<crate::AgentWakeRecoveryBatch, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[recovery_token_prefix]),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::recover_agent_wakes(
                        connection,
                        recovery_token_prefix,
                        recovered_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn cancel_agent_tree_wakes_by_root_agent(
        &self,
        root_agent_id: &str,
    ) -> Result<crate::AgentTreeWakeCancellationBatch, AgentGraphError> {
        self.cancel_agent_tree_wakes_by_root_agent_at(root_agent_id, now_ms())
    }

    pub fn cancel_agent_tree_wakes_by_root_agent_at(
        &self,
        root_agent_id: &str,
        cancelled_at: i64,
    ) -> Result<crate::AgentTreeWakeCancellationBatch, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[root_agent_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::cancel_agent_tree_wakes_by_root_agent(
                        connection,
                        root_agent_id,
                        cancelled_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn begin_agent_tree_run_stop_by_root_agent(
        &self,
        root_agent_id: &str,
        root_run_id: &str,
    ) -> Result<Option<crate::AgentTreeRunStopCancellation>, AgentGraphError> {
        self.begin_agent_tree_run_stop_by_root_agent_at(root_agent_id, root_run_id, now_ms())
    }

    pub fn begin_agent_tree_run_stop_by_root_agent_at(
        &self,
        root_agent_id: &str,
        root_run_id: &str,
        stopped_at: i64,
    ) -> Result<Option<crate::AgentTreeRunStopCancellation>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[root_agent_id, root_run_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::begin_agent_tree_run_stop_by_root_agent(
                        connection,
                        root_agent_id,
                        root_run_id,
                        stopped_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn cancel_agent_tree_wakes_by_root_conversation(
        &self,
        root_conversation_id: &str,
    ) -> Result<crate::AgentTreeWakeCancellationBatch, AgentGraphError> {
        self.cancel_agent_tree_wakes_by_root_conversation_at(root_conversation_id, now_ms())
    }

    pub fn cancel_agent_tree_wakes_by_root_conversation_at(
        &self,
        root_conversation_id: &str,
        cancelled_at: i64,
    ) -> Result<crate::AgentTreeWakeCancellationBatch, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[root_conversation_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::cancel_agent_tree_wakes_by_root_conversation(
                        connection,
                        root_conversation_id,
                        cancelled_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn begin_agent_tree_run_stop_by_root_conversation(
        &self,
        root_conversation_id: &str,
        root_run_id: &str,
    ) -> Result<Option<crate::AgentTreeRunStopCancellation>, AgentGraphError> {
        self.begin_agent_tree_run_stop_by_root_conversation_at(
            root_conversation_id,
            root_run_id,
            now_ms(),
        )
    }

    pub fn begin_agent_tree_run_stop_by_root_conversation_at(
        &self,
        root_conversation_id: &str,
        root_run_id: &str,
        stopped_at: i64,
    ) -> Result<Option<crate::AgentTreeRunStopCancellation>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[root_conversation_id, root_run_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::begin_agent_tree_run_stop_by_root_conversation(
                        connection,
                        root_conversation_id,
                        root_run_id,
                        stopped_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn reinforce_agent_tree_run_stop(
        &self,
        run_id: &str,
    ) -> Result<Option<crate::AgentTreeRunStopCancellation>, AgentGraphError> {
        self.reinforce_agent_tree_run_stop_at(run_id, now_ms())
    }

    pub fn reinforce_agent_tree_run_stop_at(
        &self,
        run_id: &str,
        cancelled_at: i64,
    ) -> Result<Option<crate::AgentTreeRunStopCancellation>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[run_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::reinforce_agent_tree_run_stop(
                        connection,
                        run_id,
                        cancelled_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn get_agent_tree_run_stop(
        &self,
        run_id: &str,
    ) -> Result<Option<crate::AgentTreeRunStopRecord>, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[run_id]),
                StorageWriteClass::Control,
                |connection| agent_graph_repository::get_agent_tree_run_stop(connection, run_id),
            )
            .map_err(unavailable)?
    }

    pub fn renew_agent_wake_lease(
        &self,
        wake_id: &str,
        claim_token: &str,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        let started = std::time::Instant::now();
        let started_at = now_ms();
        self.state
            .write_sync(
                agent_job_bytes(&[wake_id, claim_token]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::renew_agent_wake_lease_with_clock(
                        connection,
                        wake_id,
                        claim_token,
                        || {
                            let elapsed =
                                i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
                            now_ms().max(started_at.saturating_add(elapsed))
                        },
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn renew_agent_wake_lease_at(
        &self,
        wake_id: &str,
        claim_token: &str,
        renewed_at: i64,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[wake_id, claim_token]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::renew_agent_wake_lease(
                        connection,
                        wake_id,
                        claim_token,
                        renewed_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn transition_agent_wake(
        &self,
        wake_id: &str,
        expected_status: AgentWakeStatus,
        requested_status: AgentWakeStatus,
        claim_token: Option<&str>,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        let started = std::time::Instant::now();
        let started_at = now_ms();
        self.transition_agent_wake_with_clock(
            wake_id,
            expected_status,
            requested_status,
            claim_token,
            || {
                let elapsed = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
                now_ms().max(started_at.saturating_add(elapsed))
            },
        )
    }

    pub(super) fn transition_agent_wake_with_clock(
        &self,
        wake_id: &str,
        expected_status: AgentWakeStatus,
        requested_status: AgentWakeStatus,
        claim_token: Option<&str>,
        clock: impl Fn() -> i64 + Send,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[wake_id, claim_token.unwrap_or_default()]),
                StorageWriteClass::Control,
                move |connection| {
                    let transaction = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .map_err(|error| unavailable(storage_error(error)))?;
                    let original =
                        agent_graph_repository::get_agent_wake(&transaction, wake_id)?
                            .ok_or_else(|| AgentGraphError::WakeNotFound(wake_id.to_string()))?;
                    let updated = agent_graph_repository::transition_agent_wake_in_connection(
                        &transaction,
                        wake_id,
                        expected_status,
                        requested_status,
                        claim_token,
                        clock(),
                    )?;
                    // A same-state Running retry is still execution authority. The repository's
                    // idempotent fast path must not bypass the lease after writer queueing.
                    // Existing terminal no-op receipts remain replayable after their lease ends.
                    let needs_live_claim = expected_status != AgentWakeStatus::Queued
                        && (expected_status != requested_status
                            || matches!(
                                requested_status,
                                AgentWakeStatus::Claimed
                                    | AgentWakeStatus::Running
                                    | AgentWakeStatus::WaitingForApproval
                            ));
                    if needs_live_claim
                        && original
                            .lease_expires_at
                            .is_none_or(|deadline| clock() >= deadline)
                    {
                        return Err(AgentGraphError::Conflict(
                            "Wake lease expired before transition commit".into(),
                        ));
                    }
                    transaction
                        .commit()
                        .map_err(|error| unavailable(storage_error(error)))?;
                    Ok(updated)
                },
            )
            .map_err(unavailable)?
    }

    pub fn transition_agent_wake_at(
        &self,
        wake_id: &str,
        expected_status: AgentWakeStatus,
        requested_status: AgentWakeStatus,
        claim_token: Option<&str>,
        transitioned_at: i64,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[wake_id, claim_token.unwrap_or_default()]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::transition_agent_wake(
                        connection,
                        wake_id,
                        expected_status,
                        requested_status,
                        claim_token,
                        transitioned_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn finish_agent_wake_with_result(
        &self,
        input: &FinishAgentWakeWithResultInput,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        // Round-1 compatibility only. Production Dispatcher/Host code must use
        // `finish_agent_turn_with_result`; the application layer intentionally does not expose
        // this caller-shaped payload API.
        self.state
            .write_sync(
                agent_job_bytes(&[
                    &input.wake_id,
                    &input.claim_token,
                    input.terminal_error.as_deref().unwrap_or_default(),
                ])
                .saturating_add(message_job_bytes(&input.result_message)),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::finish_agent_wake_with_result(
                        connection,
                        input,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn resolve_claimed_agent_wake(
        &self,
        agent_id: &str,
        wake_id: &str,
        source_agent_message_id: &str,
        claim_token: &str,
    ) -> Result<crate::ChildAgentSpawnRecord, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[agent_id, wake_id, source_agent_message_id, claim_token]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::resolve_claimed_agent_wake_bundle(
                        connection,
                        agent_id,
                        wake_id,
                        source_agent_message_id,
                        claim_token,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn send_agent_message(
        &self,
        input: &crate::SendAgentMessageRequest,
    ) -> Result<crate::AgentMessageDispatch, AgentGraphError> {
        self.state
            .write_sync(
                send_job_bytes(input),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::send_agent_message(connection, input, now_ms())
                },
            )
            .map_err(unavailable)?
    }

    pub fn send_agent_message_from_run(
        &self,
        input: &crate::SendAgentMessageRequest,
        origin_run_id: &str,
    ) -> Result<crate::AgentMessageDispatch, AgentGraphError> {
        self.state
            .write_sync(
                send_job_bytes(input).saturating_add(origin_run_id.len().saturating_mul(2)),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::send_agent_message_from_run(
                        connection,
                        input,
                        origin_run_id,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn follow_up_agent(
        &self,
        input: &crate::SendAgentMessageRequest,
    ) -> Result<crate::AgentMessageDispatch, AgentGraphError> {
        self.state
            .write_sync(
                send_job_bytes(input),
                StorageWriteClass::Normal,
                |connection| agent_graph_repository::follow_up_agent(connection, input, now_ms()),
            )
            .map_err(unavailable)?
    }

    pub fn follow_up_agent_from_run(
        &self,
        input: &crate::SendAgentMessageRequest,
        origin_run_id: &str,
    ) -> Result<crate::AgentMessageDispatch, AgentGraphError> {
        self.state
            .write_sync(
                send_job_bytes(input).saturating_add(origin_run_id.len().saturating_mul(2)),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::follow_up_agent_from_run(
                        connection,
                        input,
                        origin_run_id,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn finish_agent_turn_with_result(
        &self,
        input: &crate::FinishAgentTurnResultInput,
    ) -> Result<crate::AgentTurnResultSettlement, AgentGraphError> {
        let started = std::time::Instant::now();
        let started_at = now_ms();
        self.state
            .write_sync(
                result_job_bytes(input),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::finish_agent_turn_with_result_with_clock(
                        connection,
                        input,
                        || {
                            let elapsed =
                                i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
                            now_ms().max(started_at.saturating_add(elapsed))
                        },
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn finish_agent_turn_with_result_at(
        &self,
        input: &crate::FinishAgentTurnResultInput,
        completed_at: i64,
    ) -> Result<crate::AgentTurnResultSettlement, AgentGraphError> {
        self.state
            .write_sync(
                result_job_bytes(input),
                StorageWriteClass::Normal,
                |connection| {
                    agent_graph_repository::finish_agent_turn_with_result(
                        connection,
                        input,
                        completed_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn settle_tree_stopped_active_wake(
        &self,
        wake: &crate::ActiveAgentTreeWake,
    ) -> Result<crate::AgentTreeStoppedWakeSettlementOutcome, AgentGraphError> {
        self.settle_tree_stopped_active_wake_at(wake, now_ms())
    }

    pub fn settle_tree_stopped_active_wake_at(
        &self,
        wake: &crate::ActiveAgentTreeWake,
        completed_at: i64,
    ) -> Result<crate::AgentTreeStoppedWakeSettlementOutcome, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[
                    &wake.agent_id,
                    &wake.conversation_id,
                    &wake.wake_id,
                    &wake.run_id,
                    &wake.assistant_message_id,
                    &wake.claim_token,
                ]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::settle_tree_stopped_active_wake(
                        connection,
                        wake,
                        completed_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn interrupt_agent_execution_at(
        &self,
        caller_agent_id: &str,
        target_agent_id: &str,
        request_id: &str,
        interrupted_at: i64,
    ) -> Result<crate::InterruptAgentExecutionOutcome, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[caller_agent_id, target_agent_id, request_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::interrupt_agent_execution(
                        connection,
                        caller_agent_id,
                        target_agent_id,
                        request_id,
                        interrupted_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn interrupt_agent_execution_with_receipt_at(
        &self,
        caller_agent_id: &str,
        target_agent_id: &str,
        request_id: &str,
        interrupted_at: i64,
    ) -> Result<crate::InterruptAgentExecutionReceipt, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[caller_agent_id, target_agent_id, request_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::interrupt_agent_execution_with_receipt(
                        connection,
                        caller_agent_id,
                        target_agent_id,
                        request_id,
                        interrupted_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn mark_agent_interrupt_dispatched_at(
        &self,
        caller_agent_id: &str,
        request_id: &str,
        dispatched_at: i64,
    ) -> Result<crate::InterruptAgentExecutionReceipt, AgentGraphError> {
        self.state
            .write_sync(
                agent_job_bytes(&[caller_agent_id, request_id]),
                StorageWriteClass::Control,
                |connection| {
                    agent_graph_repository::mark_agent_interrupt_dispatched(
                        connection,
                        caller_agent_id,
                        request_id,
                        dispatched_at,
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn list_undispatched_agent_interrupts(
        &self,
    ) -> Result<Vec<crate::UndispatchedAgentInterrupt>, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_graph_repository::list_undispatched_agent_interrupts(&connection)
    }

    pub fn get_agent_display_status(
        &self,
        agent_id: &str,
    ) -> Result<crate::AgentDisplayStatusSnapshot, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_graph_repository::get_agent_display_status(&connection, agent_id)
    }
}
