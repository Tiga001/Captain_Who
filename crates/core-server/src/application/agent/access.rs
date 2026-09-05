use super::*;

impl AgentService {
    pub(crate) fn authorize_user_attachment_reads(
        &self,
        attachment_ids: &[String],
    ) -> Result<(), AgentServiceError> {
        for attachment_id in attachment_ids {
            if let Some(conversation_id) = self.storage.attachment_conversation_id(attachment_id)? {
                self.authorize_user_conversation_write(&conversation_id)?;
            }
        }
        Ok(())
    }

    pub(crate) fn authorize_user_conversation_write(
        &self,
        conversation_id: &str,
    ) -> Result<(), AgentServiceError> {
        self.collaboration_authorizer
            .authorize_user_conversation_write(conversation_id)
            .map(|_| ())
            .map_err(|error| error.to_string().into())
    }

    pub(crate) fn authorize_exact_child_observer_read(
        &self,
        root_conversation_id: &str,
        observed_conversation_id: &str,
    ) -> Result<mycopilot_core::AgentNodeRecord, AgentServiceError> {
        self.collaboration_authorizer
            .authorize_exact_observer_read(root_conversation_id, observed_conversation_id)
            .map_err(|error| error.to_string().into())
    }

    pub(crate) fn collaboration_authorizer(
        &self,
    ) -> crate::application::collaboration_authorization::CollaborationAuthorizer {
        self.collaboration_authorizer.clone()
    }

    /// Authorizes the owner identity resolved once by the user-stop boundary. An unauthorized
    /// Conversation returns `false`, avoiding an existence oracle across Agent trees.
    pub(super) fn authorize_resolved_user_run_write(
        &self,
        conversation_id: Option<&str>,
    ) -> Result<bool, String> {
        let Some(conversation_id) = conversation_id else {
            // Preserve the legacy behaviour for synthetic/unbound run controls used by the
            // single-Agent path. A real graph-bound run is always discoverable through the
            // active, usage, pending-action, or command-session owner indexes.
            return Ok(true);
        };
        self.collaboration_authorizer
            .authorize_user_conversation_write(conversation_id)
            .map(|_| true)
            .or_else(|error| match error {
                crate::application::collaboration_authorization::CollaborationAuthorizationError::ReadOnlyChildConversation
                | crate::application::collaboration_authorization::CollaborationAuthorizationError::CallerUnavailable
                | crate::application::collaboration_authorization::CollaborationAuthorizationError::PermissionDenied => Ok(false),
                other => Err(other.to_string()),
            })
    }

    pub(super) fn authorize_user_pending_action(
        &self,
        run_id: &str,
        action_id: &str,
    ) -> Result<(), String> {
        let storage_id = pending_action_storage_id(run_id, action_id);
        let in_memory_conversation_id = {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending_actions
                .get(&storage_id)
                .and_then(|record| record.snapshot.conversation_id.clone())
        };
        let conversation_id = match in_memory_conversation_id {
            Some(conversation_id) => conversation_id,
            None => self
                .storage
                .get_pending_agent_action(&storage_id)
                .map_err(|error| error.to_string())?
                .filter(|record| record.run_id == run_id)
                .and_then(|record| record.conversation_id)
                .ok_or_else(|| {
                    "Pending action is unavailable or no longer actionable.".to_string()
                })?,
        };
        // Never hold the in-memory approval mutex while acquiring the SQLite authorization
        // connection; settlement paths persist first and then update the same map. Terminal rows
        // remain the durable owner authority after their in-memory pending entry is removed, so
        // repeated cancel/decision requests still reach the existing idempotent settlement path.
        self.collaboration_authorizer
            .authorize_user_conversation_write(&conversation_id)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(super) fn conversation_id_for_run(&self, run_id: &str) -> Option<String> {
        self.active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .map(|control| control.conversation_id.clone())
            .or_else(|| {
                self.usage_contexts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(run_id)
                    .map(|state| state.context.conversation_id.clone())
            })
            .or_else(|| {
                self.pending_actions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .values()
                    .find(|record| record.snapshot.run_id == run_id)
                    .and_then(|record| record.snapshot.conversation_id.clone())
            })
            .or_else(|| self.command_sessions.conversation_for_origin_run(run_id))
            .or_else(|| {
                self.storage
                    .load_sync_human_interaction_for_run(run_id)
                    .ok()
                    .flatten()
                    .map(|(request, _)| request.conversation_id)
            })
    }
}
