use super::StorageService;
use crate::human_interaction::*;
use crate::storage::{human_interaction_repository as repository, now_ms};
use repository::HostHumanInteractionOwner;

impl StorageService {
    pub fn load_human_interaction_settings(
        &self,
    ) -> Result<HumanInteractionSettings, HumanInteractionError> {
        let connection = self.state.connection().map_err(repository::unavailable)?;
        repository::load_settings(&connection)
    }

    pub fn update_human_interaction_settings(
        &self,
        input: &HumanInteractionSettingsUpdate,
    ) -> Result<HumanInteractionSettings, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::update_settings(&mut connection, input, now_ms())
    }

    pub fn list_human_interaction_requests(
        &self,
        input: &HumanInteractionListInput,
    ) -> Result<HumanInteractionListOutput, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::list_requests(&mut connection, input)
    }

    pub fn submit_human_interaction(
        &self,
        input: &HumanInteractionSubmitInput,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::submit(&mut connection, input, now_ms())
    }

    pub fn ignore_human_interaction(
        &self,
        input: &HumanInteractionIgnoreInput,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::ignore(&mut connection, input, now_ms())
    }

    /// Host-only admission. This native owner has no wire deserializer and this method is not RPC.
    pub fn create_human_interaction_request(
        &self,
        owner: &HostHumanInteractionOwner,
        mode: HumanInteractionMode,
        input: &HumanInteractionToolInput,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::create_request(&mut connection, owner, mode, input, now_ms())
    }

    #[allow(dead_code)]
    pub(crate) fn save_human_interaction_suspension(
        &self,
        input: &repository::HumanInteractionSuspension,
    ) -> Result<(), HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::save_suspension(&mut connection, input, now_ms())
    }

    #[allow(dead_code)]
    pub(crate) fn load_human_interaction_suspension(
        &self,
        request_id: &str,
    ) -> Result<Option<repository::HumanInteractionSuspension>, HumanInteractionError> {
        let connection = self.state.connection().map_err(repository::unavailable)?;
        repository::load_suspension(&connection, request_id)
    }
}
