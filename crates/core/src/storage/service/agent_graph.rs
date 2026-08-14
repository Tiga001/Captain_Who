use super::*;
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

impl StorageService {
    pub fn ensure_agent_conversation_deletable(
        &self,
        conversation_id: &str,
    ) -> Result<(), AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::ensure_conversation_unbound(&connection, conversation_id)
    }

    pub fn ensure_agent_project_deletable(&self, project_id: &str) -> Result<(), AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::ensure_project_unbound(&connection, project_id)
    }

    pub fn ensure_root_agent(
        &self,
        input: &EnsureRootAgentInput,
    ) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::ensure_root_agent(&mut connection, input, now_ms())
    }

    #[cfg(test)]
    pub(crate) fn create_agent_node(
        &self,
        input: &CreateAgentNodeInput,
    ) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::create_agent_node(&mut connection, input, now_ms())
    }

    pub fn get_agent_node(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::get_agent_node(&connection, agent_id)
    }

    pub fn get_agent_node_by_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::get_agent_node_by_conversation(&connection, conversation_id)
    }

    pub fn get_agent_effective_permission_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentEffectivePermissionSnapshot>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::get_agent_effective_permission_snapshot(&connection, agent_id)
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
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::record_agent_effective_permissions_for_active_turn(
            &mut connection,
            agent_id,
            conversation_id,
            run_id,
            assistant_message_id,
            permissions,
            now_ms(),
        )
    }

    pub fn list_agent_children(
        &self,
        root_agent_id: &str,
        parent_agent_id: &str,
    ) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::list_agent_children(&connection, root_agent_id, parent_agent_id)
    }

    pub fn list_agent_tree(
        &self,
        root_agent_id: &str,
    ) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::list_agent_tree(&connection, root_agent_id)
    }

    pub fn transition_agent_lifecycle(
        &self,
        agent_id: &str,
        expected_revision: u64,
        expected_lifecycle: AgentLifecycle,
        requested_lifecycle: AgentLifecycle,
    ) -> Result<AgentNodeRecord, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::transition_agent_lifecycle(
            &mut connection,
            agent_id,
            expected_revision,
            expected_lifecycle,
            requested_lifecycle,
            now_ms(),
        )
    }

    pub fn enqueue_agent_message(
        &self,
        input: &EnqueueAgentMessageInput,
    ) -> Result<IdempotentCreate<AgentMailboxMessageRecord>, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::enqueue_agent_message(&mut connection, input, now_ms())
    }

    pub fn get_agent_message(
        &self,
        message_id: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::get_agent_message(&connection, message_id)
    }

    pub fn claim_next_agent_message(
        &self,
        recipient_agent_id: &str,
        claim_token: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::claim_next_agent_message(
            &mut connection,
            recipient_agent_id,
            claim_token,
            now_ms(),
        )
    }

    pub fn renew_agent_message_lease(
        &self,
        message_id: &str,
        claim_token: &str,
    ) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::renew_agent_message_lease(
            &mut connection,
            message_id,
            claim_token,
            now_ms(),
        )
    }

    pub fn acknowledge_agent_message_with_projection(
        &self,
        message_id: &str,
        claim_token: &str,
    ) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::acknowledge_agent_message_with_projection(
            &mut connection,
            message_id,
            claim_token,
            now_ms(),
        )
    }

    pub fn acknowledge_agent_task_with_projection_and_wake(
        &self,
        input: &AcknowledgeAgentTaskAndWakeInput,
    ) -> Result<(AgentMailboxMessageRecord, AgentWakeRequestRecord), AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::acknowledge_agent_task_with_projection_and_wake(
            &mut connection,
            input,
            now_ms(),
        )
    }

    pub fn conversation_message_origin(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<ConversationMessageOrigin, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
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
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::enqueue_agent_wake(&mut connection, input, now_ms())
    }

    pub fn get_agent_wake(
        &self,
        wake_id: &str,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::get_agent_wake(&connection, wake_id)
    }

    pub fn claim_next_agent_wake(
        &self,
        agent_id: &str,
        claim_token: &str,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::claim_next_agent_wake(
            &mut connection,
            agent_id,
            claim_token,
            now_ms(),
        )
    }

    pub fn claim_next_dispatchable_agent_wake(
        &self,
        claim_token: &str,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        self.claim_next_dispatchable_agent_wake_at(claim_token, now_ms())
    }

    pub fn claim_next_dispatchable_agent_wake_at(
        &self,
        claim_token: &str,
        claimed_at: i64,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::claim_next_dispatchable_agent_wake(
            &mut connection,
            claim_token,
            claimed_at,
        )
    }

    pub fn recover_agent_wakes(
        &self,
        recovery_token_prefix: &str,
    ) -> Result<crate::AgentWakeRecoveryBatch, AgentGraphError> {
        self.recover_agent_wakes_at(recovery_token_prefix, now_ms())
    }

    pub fn recover_agent_wakes_at(
        &self,
        recovery_token_prefix: &str,
        recovered_at: i64,
    ) -> Result<crate::AgentWakeRecoveryBatch, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::recover_agent_wakes(
            &mut connection,
            recovery_token_prefix,
            recovered_at,
        )
    }

    pub fn renew_agent_wake_lease(
        &self,
        wake_id: &str,
        claim_token: &str,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        self.renew_agent_wake_lease_at(wake_id, claim_token, now_ms())
    }

    pub fn renew_agent_wake_lease_at(
        &self,
        wake_id: &str,
        claim_token: &str,
        renewed_at: i64,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::renew_agent_wake_lease(
            &mut connection,
            wake_id,
            claim_token,
            renewed_at,
        )
    }

    pub fn transition_agent_wake(
        &self,
        wake_id: &str,
        expected_status: AgentWakeStatus,
        requested_status: AgentWakeStatus,
        claim_token: Option<&str>,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        self.transition_agent_wake_at(
            wake_id,
            expected_status,
            requested_status,
            claim_token,
            now_ms(),
        )
    }

    pub fn transition_agent_wake_at(
        &self,
        wake_id: &str,
        expected_status: AgentWakeStatus,
        requested_status: AgentWakeStatus,
        claim_token: Option<&str>,
        transitioned_at: i64,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::transition_agent_wake(
            &mut connection,
            wake_id,
            expected_status,
            requested_status,
            claim_token,
            transitioned_at,
        )
    }

    pub fn finish_agent_wake_with_result(
        &self,
        input: &FinishAgentWakeWithResultInput,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        // Round-1 compatibility only. Production Dispatcher/Host code must use
        // `finish_agent_turn_with_result`; the application layer intentionally does not expose
        // this caller-shaped payload API.
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::finish_agent_wake_with_result(&mut connection, input, now_ms())
    }

    pub fn resolve_claimed_agent_wake(
        &self,
        agent_id: &str,
        wake_id: &str,
        source_agent_message_id: &str,
        claim_token: &str,
    ) -> Result<crate::ChildAgentSpawnRecord, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::resolve_claimed_agent_wake_bundle(
            &connection,
            agent_id,
            wake_id,
            source_agent_message_id,
            claim_token,
            now_ms(),
        )
    }

    pub fn send_agent_message(
        &self,
        input: &crate::SendAgentMessageRequest,
    ) -> Result<crate::AgentMessageDispatch, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::send_agent_message(&mut connection, input, now_ms())
    }

    pub fn follow_up_agent(
        &self,
        input: &crate::SendAgentMessageRequest,
    ) -> Result<crate::AgentMessageDispatch, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::follow_up_agent(&mut connection, input, now_ms())
    }

    pub fn finish_agent_turn_with_result(
        &self,
        input: &crate::FinishAgentTurnResultInput,
    ) -> Result<crate::AgentTurnResultSettlement, AgentGraphError> {
        self.finish_agent_turn_with_result_at(input, now_ms())
    }

    pub fn finish_agent_turn_with_result_at(
        &self,
        input: &crate::FinishAgentTurnResultInput,
        completed_at: i64,
    ) -> Result<crate::AgentTurnResultSettlement, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::finish_agent_turn_with_result(&mut connection, input, completed_at)
    }

    pub fn interrupt_agent_execution_at(
        &self,
        caller_agent_id: &str,
        target_agent_id: &str,
        request_id: &str,
        interrupted_at: i64,
    ) -> Result<crate::InterruptAgentExecutionOutcome, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::interrupt_agent_execution(
            &mut connection,
            caller_agent_id,
            target_agent_id,
            request_id,
            interrupted_at,
        )
    }

    pub fn get_agent_display_status(
        &self,
        agent_id: &str,
    ) -> Result<crate::AgentDisplayStatusSnapshot, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::get_agent_display_status(&connection, agent_id)
    }
}
