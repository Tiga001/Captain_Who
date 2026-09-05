//! Host entry points for durable human questions. This layer only acknowledges stored facts;
//! synchronous continuation and asynchronous delivery are intentionally not implemented in round 1.

use crate::application::agent::{AgentService, CoreServerNotificationSender};
use mycopilot_core::human_interaction::*;
use mycopilot_core::storage::service::StorageService;
use mycopilot_protocol_rs::{
    HUMAN_INTERACTION_REQUEST_CHANGED_METHOD, HUMAN_INTERACTION_SETTINGS_CHANGED_METHOD,
};
use serde::Serialize;
use std::sync::Arc;

pub(crate) struct StoredHumanInteractionPolicy(pub(crate) Arc<StorageService>);

impl mycopilot_core::HumanInteractionPolicySource for StoredHumanInteractionPolicy {
    fn snapshot(&self) -> mycopilot_core::AgentResult<HumanInteractionSettings> {
        self.0
            .load_human_interaction_settings()
            .map_err(|_| mycopilot_core::AgentError::new("无法读取人机交互设置。"))
    }
}

pub(crate) struct HumanInteractionService<'a> {
    storage: &'a StorageService,
    agent: &'a AgentService,
}

impl<'a> HumanInteractionService<'a> {
    pub(crate) fn new(storage: &'a StorageService, agent: &'a AgentService) -> Self {
        Self { storage, agent }
    }

    pub(crate) fn get_settings(&self) -> Result<HumanInteractionSettings, HumanInteractionError> {
        self.storage.load_human_interaction_settings()
    }

    pub(crate) fn update_settings(
        &self,
        input: HumanInteractionSettingsUpdate,
        notifications: &CoreServerNotificationSender,
    ) -> Result<HumanInteractionSettings, HumanInteractionError> {
        if input.expected_revision > HUMAN_INTERACTION_MAX_SAFE_INTEGER {
            return Err(HumanInteractionError::invalid());
        }
        let settings = self.storage.update_human_interaction_settings(&input)?;
        emit(
            notifications,
            HUMAN_INTERACTION_SETTINGS_CHANGED_METHOD,
            &settings,
        );
        Ok(settings)
    }

    pub(crate) fn list(
        &self,
        input: HumanInteractionListInput,
    ) -> Result<HumanInteractionListOutput, HumanInteractionError> {
        self.authorize(&input.conversation_id)?;
        if !(1..=100).contains(&input.limit)
            || input
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.len() > 2048 || cursor.trim().is_empty())
        {
            return Err(HumanInteractionError::invalid());
        }
        self.storage.list_human_interaction_requests(&input)
    }

    pub(crate) fn submit(
        &self,
        input: HumanInteractionSubmitInput,
        notifications: &CoreServerNotificationSender,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        self.authorize(&input.conversation_id)?;
        validate_mutation(
            &input.request_id,
            input.expected_revision,
            &input.submission_id,
        )?;
        let snapshot = self.storage.submit_human_interaction(&input)?;
        emit(
            notifications,
            HUMAN_INTERACTION_REQUEST_CHANGED_METHOD,
            &snapshot,
        );
        Ok(snapshot)
    }

    pub(crate) fn ignore(
        &self,
        input: HumanInteractionIgnoreInput,
        notifications: &CoreServerNotificationSender,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        self.authorize(&input.conversation_id)?;
        validate_mutation(
            &input.request_id,
            input.expected_revision,
            &input.submission_id,
        )?;
        let snapshot = self.storage.ignore_human_interaction(&input)?;
        emit(
            notifications,
            HUMAN_INTERACTION_REQUEST_CHANGED_METHOD,
            &snapshot,
        );
        Ok(snapshot)
    }

    fn authorize(&self, conversation_id: &str) -> Result<(), HumanInteractionError> {
        validate_human_interaction_id(conversation_id)?;
        self.agent
            .authorize_user_conversation_write(conversation_id)
            .map_err(|_| {
                HumanInteractionError::new(
                    "access_denied",
                    "This conversation does not allow human interaction.",
                )
            })
    }
}

fn validate_mutation(
    request_id: &str,
    revision: u64,
    submission_id: &str,
) -> Result<(), HumanInteractionError> {
    validate_human_interaction_id(request_id)?;
    validate_human_interaction_id(submission_id)?;
    if revision > HUMAN_INTERACTION_MAX_SAFE_INTEGER {
        return Err(HumanInteractionError::invalid());
    }
    Ok(())
}

fn emit<T: Serialize>(notifications: &CoreServerNotificationSender, method: &str, snapshot: &T) {
    // Queries remain authoritative if a disconnected renderer misses this post-commit signal.
    let _ = notifications
        .send(serde_json::json!({"jsonrpc":"2.0", "method":method, "params":snapshot}));
}
