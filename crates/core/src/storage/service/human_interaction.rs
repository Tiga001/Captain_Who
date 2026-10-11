use super::StorageService;
use crate::human_interaction::*;
use crate::storage::{human_interaction_repository as repository, now_ms};
use repository::HostHumanInteractionOwner;

impl StorageService {
    pub fn load_human_interaction_attention(
        &self,
    ) -> Result<HumanInteractionAttentionSnapshot, HumanInteractionError> {
        let mut connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        repository::load_attention(&mut connection)
    }

    pub fn get_human_interaction_request(
        &self,
        conversation_id: &str,
        request_id: &str,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        let connection = connection
            .unchecked_transaction()
            .map_err(repository::unavailable)?;
        repository::load_request(&connection, conversation_id, request_id)
    }

    pub fn bind_human_interaction_ignored_at_sampling(
        &self,
        request: &crate::AgentSamplingBoundaryRequest,
    ) -> Result<Vec<crate::AgentHumanInteractionIgnoredEvent>, HumanInteractionError> {
        self.state
            .write_sync(
                request.conversation_id.len()
                    + request.run_id.len()
                    + request.assistant_message_id.len()
                    + 1024,
                crate::storage::StorageWriteClass::Control,
                |connection| repository::bind_ignored_at_sampling(connection, request, now_ms()),
            )
            .map_err(repository::unavailable)?
    }
    pub fn load_human_interaction_settings(
        &self,
    ) -> Result<HumanInteractionSettings, HumanInteractionError> {
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        repository::load_settings(&connection)
    }

    pub fn update_human_interaction_settings(
        &self,
        input: &HumanInteractionSettingsUpdate,
    ) -> Result<HumanInteractionSettings, HumanInteractionError> {
        self.state
            .write_sync(
                estimated(input)?.saturating_add(4096),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::update_settings(connection, input, now_ms()),
            )
            .map_err(repository::unavailable)?
    }

    pub fn list_human_interaction_requests(
        &self,
        input: &HumanInteractionListInput,
    ) -> Result<HumanInteractionListOutput, HumanInteractionError> {
        let mut connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        repository::list_requests(&mut connection, input)
    }

    pub fn submit_human_interaction(
        &self,
        input: &HumanInteractionSubmitInput,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        self.state
            .write_sync(
                estimated(input)?.saturating_add(4096),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::submit(connection, input, now_ms()),
            )
            .map_err(repository::unavailable)?
    }

    pub fn ignore_human_interaction(
        &self,
        input: &HumanInteractionIgnoreInput,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        self.state
            .write_sync(
                estimated(input)?.saturating_add(4096),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::ignore(connection, input, now_ms()),
            )
            .map_err(repository::unavailable)?
    }

    /// Host-only admission. This native owner has no wire deserializer and this method is not RPC.
    pub fn create_human_interaction_request(
        &self,
        owner: &HostHumanInteractionOwner,
        mode: HumanInteractionMode,
        input: &HumanInteractionToolInput,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        self.state
            .write_sync(
                estimated(input)?.saturating_add(4096),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::create_request(connection, owner, mode, input, now_ms()),
            )
            .map_err(repository::unavailable)?
    }

    #[allow(dead_code)]
    pub(crate) fn save_human_interaction_suspension(
        &self,
        input: &repository::HumanInteractionSuspension,
    ) -> Result<(), HumanInteractionError> {
        self.state
            .write_sync(
                estimated(&input.checkpoint)?.saturating_add(
                    input.request_id.len()
                        + input.run_id.len()
                        + input.assistant_message_id.len()
                        + input.tool_call_id.len(),
                ),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::save_suspension(connection, input, now_ms()),
            )
            .map_err(repository::unavailable)?
    }

    #[allow(dead_code)]
    pub(crate) fn load_human_interaction_suspension(
        &self,
        request_id: &str,
    ) -> Result<Option<repository::HumanInteractionSuspension>, HumanInteractionError> {
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        repository::load_suspension(&connection, request_id)
    }
}

impl StorageService {
    pub fn admit_sync_human_interaction(
        &self,
        input: &repository::HumanInteractionSyncAdmission,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        self.state
            .write_sync(
                estimated(&(&input.input, &input.checkpoint))?.saturating_add(
                    input.content.len()
                        + input
                            .pending_action_predecessor
                            .as_ref()
                            .map_or(0, |p| p.terminal_agent_input_json.len())
                        + 8192,
                ),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::admit_sync(connection, input, now_ms()),
            )
            .map_err(repository::unavailable)?
    }
    pub fn claim_sync_human_interaction(
        &self,
        request_id: &str,
    ) -> Result<Option<repository::HumanInteractionSyncResume>, HumanInteractionError> {
        self.state
            .write_sync(
                request_id.len().saturating_add(1024),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::claim_sync(connection, request_id, now_ms()),
            )
            .map_err(repository::unavailable)?
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
        self.state
            .write_sync(
                4096,
                crate::storage::StorageWriteClass::Control,
                |connection| repository::advance_sync(connection, binding, transition, now_ms()),
            )
            .map_err(repository::unavailable)?
    }
    pub fn cancel_sync_human_interactions_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        self.state
            .write_sync(
                run_id.len().saturating_add(1024),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::cancel_sync_for_run(connection, run_id, now_ms()),
            )
            .map_err(repository::unavailable)?
    }
    pub fn reconcile_sync_human_interactions(
        &self,
    ) -> Result<Vec<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        self.state
            .write_sync(
                4096,
                crate::storage::StorageWriteClass::Control,
                |connection| repository::reconcile_sync(connection, now_ms()),
            )
            .map_err(repository::unavailable)?
    }
    pub fn list_sync_human_interaction_ready_resumes(
        &self,
    ) -> Result<Vec<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        let connection = connection
            .unchecked_transaction()
            .map_err(repository::unavailable)?;
        repository::list_sync_ready(&connection)
    }
    pub fn has_sync_human_interaction_wait(
        &self,
        conversation_id: &str,
    ) -> Result<bool, HumanInteractionError> {
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        let connection = connection
            .unchecked_transaction()
            .map_err(repository::unavailable)?;
        repository::has_sync_wait(&connection, conversation_id)
    }
    pub fn is_sync_human_interaction_run_waiting(
        &self,
        run_id: &str,
    ) -> Result<bool, HumanInteractionError> {
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        let connection = connection
            .unchecked_transaction()
            .map_err(repository::unavailable)?;
        repository::is_sync_run_waiting(&connection, run_id)
    }
}

impl StorageService {
    pub fn list_sync_human_interaction_waits(
        &self,
    ) -> Result<Vec<(HumanInteractionRequestSnapshot, serde_json::Value)>, HumanInteractionError>
    {
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        let connection = connection
            .unchecked_transaction()
            .map_err(repository::unavailable)?;
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
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        let connection = connection
            .unchecked_transaction()
            .map_err(repository::unavailable)?;
        repository::load_sync_for_run(&connection, run_id)
    }
}

impl StorageService {
    pub fn admit_async_human_interaction(
        &self,
        owner: &repository::HostHumanInteractionOwner,
        input: &HumanInteractionToolInput,
    ) -> Result<HumanInteractionRequestSnapshot, HumanInteractionError> {
        self.state
            .write_sync(
                estimated(input)?.saturating_add(4096),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::admit_async(connection, owner, input, now_ms()),
            )
            .map_err(repository::unavailable)?
    }
}

impl StorageService {
    pub fn list_pending_async_human_interaction_deliveries(
        &self,
    ) -> Result<Vec<repository::HumanInteractionAsyncPending>, HumanInteractionError> {
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        let connection = connection
            .unchecked_transaction()
            .map_err(repository::unavailable)?;
        repository::list_pending_async(&connection)
    }
    pub fn bind_async_human_interaction_to_guidance(
        &self,
        response_id: &str,
        record: &crate::storage::models::AgentRunGuidanceRecord,
        expected_delivery_revision: u64,
    ) -> Result<Option<repository::HumanInteractionAsyncBinding>, HumanInteractionError> {
        self.state
            .write_sync(
                record.content.len().saturating_add(
                    response_id.len()
                        + record.folder_references_json.len()
                        + record.attachment_ids.iter().map(String::len).sum::<usize>()
                        + 4096,
                ),
                crate::storage::StorageWriteClass::Control,
                |connection| {
                    repository::bind_async_to_guidance(
                        connection,
                        response_id,
                        record,
                        expected_delivery_revision,
                        now_ms(),
                    )
                },
            )
            .map_err(repository::unavailable)?
    }
    pub fn load_async_human_interaction_binding_for_run(
        &self,
        run_id: &str,
    ) -> Result<Option<repository::HumanInteractionAsyncBinding>, HumanInteractionError> {
        let connection = self
            .state
            .read_connection()
            .map_err(repository::unavailable)?;
        let connection = connection
            .unchecked_transaction()
            .map_err(repository::unavailable)?;
        repository::load_async_binding_for_run(&connection, run_id)
    }
    pub fn mark_async_human_interaction_turn_started(
        &self,
        binding: &repository::HumanInteractionAsyncBinding,
    ) -> Result<bool, HumanInteractionError> {
        self.state
            .write_sync(
                4096,
                crate::storage::StorageWriteClass::Control,
                |connection| repository::start_async_turn(connection, binding, now_ms()),
            )
            .map_err(repository::unavailable)?
    }
    pub fn settle_async_human_interaction_start_failure(
        &self,
        run_id: &str,
    ) -> Result<Option<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        self.state
            .write_sync(
                run_id.len().saturating_add(1024),
                crate::storage::StorageWriteClass::Control,
                |connection| repository::settle_async_start_failure(connection, run_id, now_ms()),
            )
            .map_err(repository::unavailable)?
    }
    pub fn reconcile_async_human_interaction_deliveries(
        &self,
    ) -> Result<Vec<HumanInteractionRequestSnapshot>, HumanInteractionError> {
        self.state
            .write_sync(
                4096,
                crate::storage::StorageWriteClass::Control,
                |connection| repository::reconcile_async(connection, now_ms()),
            )
            .map_err(repository::unavailable)?
    }
}

fn estimated(value: &(impl serde::Serialize + ?Sized)) -> Result<usize, HumanInteractionError> {
    crate::storage::write_size::estimated_json_bytes(value).map_err(repository::unavailable)
}
