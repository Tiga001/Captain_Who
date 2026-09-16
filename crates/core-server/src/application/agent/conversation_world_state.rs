//! Host-bound, transactional publication of state adopted by an Agent sampling boundary.
use super::*;
use mycopilot_core::storage::world_state_repository::ConversationWorldStateCommitRequest;
use mycopilot_core::{
    AgentConversationWorldStateHost, AgentConversationWorldStateRequest, AnchoredWorldStateRecord,
    WorldStateLifetime, WorldStateRequestBoundary, WorldStateSectionId,
};

struct StoredConversationWorldState {
    service: AgentService,
    input: AgentChatInput,
    run_id: String,
    conversation_id: String,
    assistant_message_id: String,
    cancellation: AgentCancellationToken,
}

impl StoredConversationWorldState {
    fn validate_boundary(&self, boundary: &WorldStateRequestBoundary) -> AgentResult<()> {
        if boundary.run_id != self.run_id
            || boundary.assistant_message_id != self.assistant_message_id
        {
            return Err(AgentError::new(
                "World State request ownership does not match its Host binding.",
            ));
        }
        Ok(())
    }

    fn refresh_cached_state(&self, records: &[AnchoredWorldStateRecord]) {
        let mut states = self
            .service
            .conversation_context_states
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let invalid = states.get_mut(&self.conversation_id).is_some_and(|entry| {
            entry
                .state
                .sync_conversation_world_state_records(records)
                .is_err()
        });
        if invalid {
            // The authoritative transaction already committed. A stale derived epoch is rebuilt
            // from SQLite by the next trace observer rather than replaying the model or its tools.
            states.remove(&self.conversation_id);
        }
    }
}

impl AgentConversationWorldStateHost for StoredConversationWorldState {
    fn prepare_request(
        &self,
        request: AgentConversationWorldStateRequest,
    ) -> AgentResult<Vec<AnchoredWorldStateRecord>> {
        self.cancellation.check()?;
        self.validate_boundary(&request.boundary)?;
        if request.conversation_id != self.conversation_id {
            return Err(AgentError::new(
                "World State conversation ownership does not match.",
            ));
        }
        let mut owned_section_ids = [
            "web.search",
            "human.interaction",
            "agent.collaboration",
            "builtin.capabilities.policy",
        ]
        .into_iter()
        .map(WorldStateSectionId::extension)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AgentError::new(e.to_string()))?;
        if request.sections.iter().any(|section| {
            section.lifetime != WorldStateLifetime::Conversation
                || !owned_section_ids.contains(&section.id)
        }) {
            return Err(AgentError::new(
                "World State capability owner attempted to replace another owner's section.",
            ));
        }
        let mut sections = crate::application::agent_support::conversation_world_state_sections(
            self.input.context.as_ref(),
            self.input.prompt_preferences.as_ref(),
            &self.input.model,
            self.input.model_capabilities,
        )
        .map_err(AgentError::new)?;
        // The Host keeps `workspace.instructions` owned on every sampling boundary even when no
        // AGENTS.md exists: ownership without a replacement is what turns a removed file into an
        // explicit Remove instead of leaving stale instructions in the committed state.
        owned_section_ids.push(WorldStateSectionId::WorkspaceInstructions);
        owned_section_ids.extend(
            sections
                .iter()
                .map(|section| section.id.clone())
                .filter(|section_id| section_id != &WorldStateSectionId::WorkspaceInstructions),
        );
        sections.extend(request.sections);
        let expected = self
            .service
            .storage
            .fold_active_conversation_world_state(&self.conversation_id)
            .map_err(AgentError::new)?;
        let epoch_id = format!("world-state:{}", self.run_id);
        let committed = self
            .service
            .storage
            .commit_conversation_world_state_request(&ConversationWorldStateCommitRequest {
                conversation_id: &self.conversation_id,
                boundary: &request.boundary,
                expected_head: expected.as_ref(),
                owned_section_ids: &owned_section_ids,
                sections: &sections,
                initial_epoch_id: &epoch_id,
                created_at: mycopilot_core::storage::now_ms(),
            })
            .map_err(AgentError::new)?;
        self.refresh_cached_state(&committed.records);
        Ok(committed.records)
    }

    fn mark_request_observed(&self, boundary: &WorldStateRequestBoundary) -> AgentResult<()> {
        self.validate_boundary(boundary)?;
        self.service
            .storage
            .mark_conversation_world_state_request_observed(&self.conversation_id, boundary)
            .map_err(AgentError::new)?;
        let records = crate::application::agent_support::load_conversation_world_state(
            &self.service.storage,
            &self.conversation_id,
        )
        .map_err(AgentError::new)?;
        self.refresh_cached_state(&records);
        Ok(())
    }
}

impl AgentService {
    pub(super) fn conversation_world_state_host(
        &self,
        input: &AgentChatInput,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        cancellation: &AgentCancellationToken,
    ) -> Arc<dyn AgentConversationWorldStateHost> {
        Arc::new(StoredConversationWorldState {
            service: self.clone(),
            input: input.clone(),
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            cancellation: cancellation.clone(),
        })
    }
}
