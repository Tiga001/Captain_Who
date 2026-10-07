use super::StorageService;
use crate::storage::workflow_execution_repository as repository;
use crate::workflow_execution::*;

impl StorageService {
    pub fn workflow_mail_receipt(
        &self,
        conversation_id: &str,
        run_id: &str,
        tool_call_id: &str,
        call: &crate::WorkflowMailReceiptCall,
    ) -> Result<Option<crate::WorkflowMailReceipt>, String> {
        repository::mail_receipt_for_call(
            &*self.state.connection()?,
            conversation_id,
            run_id,
            tool_call_id,
            call,
        )
    }
    pub fn organization_edit_receipt(
        &self,
        conversation_id: &str,
        run_id: &str,
        tool_call_id: &str,
        input: &serde_json::Value,
    ) -> Result<Option<crate::organization_personnel::Receipt>, String> {
        crate::storage::organization_personnel_repository::receipt_for_model_call(
            &*self.state.connection()?,
            conversation_id,
            run_id,
            tool_call_id,
            input,
        )
    }
    pub fn organization_edit(
        &self,
        request: &crate::organization_personnel::Request,
    ) -> Result<crate::organization_personnel::Receipt, String> {
        // Keep the exact credential-aware model directory stable until commit. Model settings
        // writers use this same coordinator; a model cannot become unavailable between this
        // validation and the organization mutation because of an in-app settings update.
        let _models_guard = self
            .model_credential_lock
            .lock()
            .map_err(|_| "model credential coordinator is unavailable".to_string())?;
        let stored = crate::storage::config_repository::load_model_settings_snapshot(
            &mut *self.state.connection()?,
        )
        .map_err(|error| error.to_string())?;
        let available_models = stored
            .as_ref()
            .map(|stored| self.model_projection_from_stored(stored))
            .into_iter()
            .flat_map(|projection| projection.models)
            .filter(|entry| entry.execution.is_available())
            .map(|entry| entry.model.id)
            .collect();
        crate::storage::organization_personnel_repository::manage(
            &mut *self.state.connection()?,
            request,
            &available_models,
        )
    }
    pub fn workflow_execution_mutate(
        &self,
        request: &MutationRequest,
    ) -> Result<serde_json::Value, String> {
        repository::mutate(&mut *self.state.connection()?, request)
    }
    pub fn workflow_execution_settle_run(&self, run_id: &str, status: &str) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        repository::settle_run(&transaction, run_id, status)?;
        transaction.commit().map_err(|e| e.to_string())
    }

    pub fn workflow_execution_awareness_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<serde_json::Value, String> {
        repository::awareness_for_conversation(&*self.state.connection()?, conversation_id)
    }
    pub fn workflow_execution_state_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
        query: &crate::workflow_awareness::StateQuery,
    ) -> Result<serde_json::Value, String> {
        repository::state_for_run(&*self.state.connection()?, conversation_id, run_id, query)
    }
    pub fn workflow_execution_mailbox_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
        query: &crate::workflow_awareness::MailboxQuery,
    ) -> Result<serde_json::Value, String> {
        repository::mailbox_for_run(&*self.state.connection()?, conversation_id, run_id, query)
    }
    pub fn workflow_execution_awareness_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<serde_json::Value, String> {
        repository::awareness_for_run(&*self.state.connection()?, conversation_id, run_id)
    }
    pub fn workflow_execution_mark_run_unread(&self, run_id: &str) -> Result<(), String> {
        repository::mark_run_unread(&mut *self.state.connection()?, run_id)
    }

    pub fn workflow_execution_snapshot_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Option<ConversationSnapshot>, String> {
        repository::snapshot_for_run(&*self.state.connection()?, conversation_id, run_id)
    }
    /// Renderer consumption positions are derived from committed trace and mailbox provenance.
    pub fn workflow_execution_delivery_presentations(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> Result<Vec<DeliveryPresentation>, String> {
        self.workflow_execution_delivery_presentations_since(
            conversation_id,
            assistant_message_id,
            None,
        )
    }

    pub fn workflow_execution_delivery_presentations_since(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        after_sequence: Option<u64>,
    ) -> Result<Vec<DeliveryPresentation>, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let result = (|| -> Result<Vec<DeliveryPresentation>, String> {
            let Some(trace) =
                crate::storage::conversation_trace_repository::workflow_delivery_trace_since(
                    &transaction,
                    conversation_id,
                    assistant_message_id,
                    after_sequence,
                )
                .map_err(|error| error.to_string())?
            else {
                return Ok(vec![]);
            };
            let input_ids = trace
                .items
                .iter()
                .filter_map(|item| match item {
                    crate::ConversationTurnTraceItem::WorkflowDelivery {
                        input_id,
                        truncated: false,
                        ..
                    } => Some(input_id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if input_ids.is_empty() {
                return Ok(vec![]);
            }
            let input_ids = serde_json::to_string(&input_ids).map_err(|error| error.to_string())?;
            let origins = repository::delivery_origins_for_input_scope(
                &transaction,
                conversation_id,
                Some(&input_ids),
            )?;
            // Include every matching origin, even before the assistant or across an observer
            // page boundary: the shared projector must still reject startup/ambiguous evidence.
            let message_ids = std::iter::once(assistant_message_id)
                .chain(origins.iter().map(|(id, _)| id.as_str()))
                .collect::<Vec<_>>();
            let message_ids =
                serde_json::to_string(&message_ids).map_err(|error| error.to_string())?;
            let Some(conversation) =
                crate::storage::chat_repository::get_active_conversation_in_message_scope(
                    &transaction,
                    conversation_id,
                    &message_ids,
                )
                .map_err(|error| error.to_string())?
            else {
                return Ok(vec![]);
            };
            Ok(repository::delivery_presentations(
                &conversation,
                &trace,
                &origins,
            ))
        })()?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(result)
    }

    pub fn workflow_execution_delivery_origins(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<(String, Input)>, String> {
        repository::delivery_origins_for_conversation(&*self.state.connection()?, conversation_id)
    }

    pub fn workflow_execution_recover_claims(&self) -> Result<(), String> {
        repository::recover_claims(&mut *self.state.connection()?)
    }
    pub fn workflow_execution_bind_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Option<ConversationSnapshot>, String> {
        repository::bind_run(&mut *self.state.connection()?, conversation_id, run_id)
    }
    pub fn workflow_execution_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationSnapshot>, String> {
        repository::snapshot_for_conversation(&*self.state.connection()?, conversation_id)
    }
    pub fn workflow_execution_send(&self, request: &SendRequest) -> Result<SendReceipt, String> {
        repository::send(&mut *self.state.connection()?, request)
    }
    pub fn workflow_execution_pending_inputs(&self) -> Result<Vec<Input>, String> {
        repository::pending_inputs(&*self.state.connection()?)
    }
    pub fn workflow_execution_pending_candidates(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<PendingInputCandidate>, String> {
        repository::pending_candidates(&*self.state.connection()?, after_sequence, limit)
    }
    pub fn workflow_execution_eligible_pending_input(
        &self,
        input_id: &str,
    ) -> Result<Option<Input>, String> {
        repository::eligible_pending_input(&*self.state.connection()?, input_id)
    }

    pub fn workflow_execution_bound_inputs(&self, run_id: &str) -> Result<Vec<Input>, String> {
        repository::bound_inputs(&*self.state.connection()?, run_id)
    }
    pub fn workflow_execution_load_input(&self, input_id: &str) -> Result<Option<Input>, String> {
        repository::load_input(&*self.state.connection()?, input_id)
    }
    pub fn workflow_execution_inputs_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<Input>, String> {
        repository::inputs_for_conversation(&*self.state.connection()?, conversation_id)
    }
    pub fn workflow_execution_bind_input(
        &self,
        input_id: &str,
        run_id: &str,
        delivery_id: &str,
    ) -> Result<bool, String> {
        repository::bind_input(
            &mut *self.state.connection()?,
            input_id,
            run_id,
            delivery_id,
        )
    }
    pub fn workflow_execution_mark_applied(&self, input_id: &str) -> Result<(), String> {
        repository::mark_applied(&mut *self.state.connection()?, input_id)
    }
    pub fn workflow_execution_fail_input(
        &self,
        input_id: &str,
        reason: &str,
    ) -> Result<(), String> {
        repository::fail_input(&mut *self.state.connection()?, input_id, reason)
    }
    pub fn workflow_execution_pause_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<(), String> {
        repository::pause_conversation(&mut *self.state.connection()?, conversation_id)
    }
    pub fn workflow_execution_resume_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<(), String> {
        repository::resume_conversation(&mut *self.state.connection()?, conversation_id)
    }
    pub fn workflow_execution_runtime(&self, instance_id: &str) -> Result<RuntimeSnapshot, String> {
        repository::runtime_snapshot(&*self.state.connection()?, instance_id, None)
    }
    pub fn workflow_execution_runtime_since(
        &self,
        instance_id: &str,
        after_sequence: Option<u64>,
    ) -> Result<RuntimeSnapshot, String> {
        repository::runtime_snapshot(&*self.state.connection()?, instance_id, after_sequence)
    }
}
