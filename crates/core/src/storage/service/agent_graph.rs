use super::*;
use crate::{
    AcknowledgeAgentTaskAndWakeInput, AgentGraphError, AgentLifecycle, AgentMailboxMessageRecord,
    AgentNodeRecord, AgentWakeRequestRecord, AgentWakeStatus, ConversationMessageOrigin,
    CreateAgentNodeInput, EnqueueAgentMessageInput, EnqueueAgentWakeInput, EnsureRootAgentInput,
    FinishAgentWakeWithResultInput, IdempotentCreate,
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

    pub fn create_agent_node(
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

    pub fn renew_agent_wake_lease(
        &self,
        wake_id: &str,
        claim_token: &str,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::renew_agent_wake_lease(
            &mut connection,
            wake_id,
            claim_token,
            now_ms(),
        )
    }

    pub fn transition_agent_wake(
        &self,
        wake_id: &str,
        expected_status: AgentWakeStatus,
        requested_status: AgentWakeStatus,
        claim_token: Option<&str>,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::transition_agent_wake(
            &mut connection,
            wake_id,
            expected_status,
            requested_status,
            claim_token,
            now_ms(),
        )
    }

    pub fn finish_agent_wake_with_result(
        &self,
        input: &FinishAgentWakeWithResultInput,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_graph_repository::finish_agent_wake_with_result(&mut connection, input, now_ms())
    }
}
