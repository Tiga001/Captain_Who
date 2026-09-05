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

impl StorageService {
    pub fn admit_sync_human_interaction(
        &self,
        input: &repository::HumanInteractionSyncAdmission,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::admit_sync(&mut connection, input, now_ms())
    }
    pub fn claim_sync_human_interaction(
        &self,
        request_id: &str,
    ) -> Result<Option<repository::HumanInteractionSyncResume>, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::claim_sync(&mut connection, request_id, now_ms())
    }
    pub fn mark_sync_human_interaction_execution_started(
        &self,
        binding: &repository::HumanInteractionSyncBinding,
    ) -> Result<bool, HumanInteractionError> {
        self.advance_sync_human_interaction(
            binding,
            repository::HumanInteractionSyncTransition::ExecutionStarted,
        )
    }
    pub fn mark_sync_human_interaction_model_in_flight(
        &self,
        binding: &repository::HumanInteractionSyncBinding,
    ) -> Result<bool, HumanInteractionError> {
        self.advance_sync_human_interaction(
            binding,
            repository::HumanInteractionSyncTransition::ModelInFlight,
        )
    }
    pub fn mark_sync_human_interaction_applied(
        &self,
        binding: &repository::HumanInteractionSyncBinding,
    ) -> Result<bool, HumanInteractionError> {
        self.advance_sync_human_interaction(
            binding,
            repository::HumanInteractionSyncTransition::Applied,
        )
    }
    fn advance_sync_human_interaction(
        &self,
        binding: &repository::HumanInteractionSyncBinding,
        transition: repository::HumanInteractionSyncTransition,
    ) -> Result<bool, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::advance_sync(&mut connection, binding, transition, now_ms())
    }
    pub fn cancel_sync_human_interactions_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::cancel_sync_for_run(&mut connection, run_id, now_ms())
    }
    pub fn reconcile_sync_human_interactions(
        &self,
    ) -> Result<Vec<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::reconcile_sync(&mut connection, now_ms())
    }
    pub fn list_sync_human_interaction_ready_resumes(
        &self,
    ) -> Result<Vec<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        let connection = self.state.connection().map_err(repository::unavailable)?;
        repository::list_sync_ready(&connection)
    }
    pub fn has_sync_human_interaction_wait(
        &self,
        conversation_id: &str,
    ) -> Result<bool, HumanInteractionError> {
        let connection = self.state.connection().map_err(repository::unavailable)?;
        repository::has_sync_wait(&connection, conversation_id)
    }
    pub fn is_sync_human_interaction_run_waiting(
        &self,
        run_id: &str,
    ) -> Result<bool, HumanInteractionError> {
        let connection = self.state.connection().map_err(repository::unavailable)?;
        repository::is_sync_run_waiting(&connection, run_id)
    }
}

impl StorageService {
    pub fn list_sync_human_interaction_waits(
        &self,
    ) -> Result<Vec<(HumanInteractionRequestSnapshot, serde_json::Value)>, HumanInteractionError>
    {
        let connection = self.state.connection().map_err(repository::unavailable)?;
        repository::list_sync_waits(&connection)
    }
}

impl StorageService {
    /// Cancellation may already have committed its stop trigger; retain access to its checkpoint.
    pub fn load_sync_human_interaction_for_run(
        &self,
        run_id: &str,
    ) -> Result<Option<(HumanInteractionRequestSnapshot, serde_json::Value)>, HumanInteractionError>
    {
        let connection = self.state.connection().map_err(repository::unavailable)?;
        repository::load_sync_for_run(&connection, run_id)
    }
}

impl StorageService {
    pub fn admit_async_human_interaction(
        &self,
        owner: &repository::HostHumanInteractionOwner,
        input: &HumanInteractionToolInput,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::admit_async(&mut connection, owner, input, now_ms())
    }
}

impl StorageService {
    pub fn list_pending_async_human_interaction_deliveries(
        &self,
    ) -> Result<Vec<repository::HumanInteractionAsyncPending>, HumanInteractionError> {
        let connection = self.state.connection().map_err(repository::unavailable)?;
        repository::list_pending_async(&connection)
    }
    pub fn bind_async_human_interaction_to_guidance(
        &self,
        response_id: &str,
        record: &crate::storage::models::AgentRunGuidanceRecord,
        expected_delivery_revision: u64,
    ) -> Result<Option<repository::HumanInteractionAsyncBinding>, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::bind_async_to_guidance(
            &mut connection,
            response_id,
            record,
            expected_delivery_revision,
            now_ms(),
        )
    }
    pub fn load_async_human_interaction_binding_for_run(
        &self,
        run_id: &str,
    ) -> Result<Option<repository::HumanInteractionAsyncBinding>, HumanInteractionError> {
        let connection = self.state.connection().map_err(repository::unavailable)?;
        repository::load_async_binding_for_run(&connection, run_id)
    }
    pub fn mark_async_human_interaction_turn_started(
        &self,
        binding: &repository::HumanInteractionAsyncBinding,
    ) -> Result<bool, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::start_async_turn(&mut connection, binding, now_ms())
    }
    pub fn settle_async_human_interaction_start_failure(
        &self,
        run_id: &str,
    ) -> Result<Option<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::settle_async_start_failure(&mut connection, run_id, now_ms())
    }
    pub fn reconcile_async_human_interaction_deliveries(
        &self,
    ) -> Result<Vec<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        let mut connection = self.state.connection().map_err(repository::unavailable)?;
        repository::reconcile_async(&mut connection, now_ms())
    }
}
