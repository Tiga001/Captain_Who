use super::StorageService;
use crate::storage::{
    workflow_execution_repository as repository, write_size::estimated_json_bytes,
    StorageWriteClass,
};
use crate::workflow_execution::*;

fn workflow_job_bytes(parts: &[&str]) -> usize {
    parts.iter().fold(512_usize, |bytes, part| {
        bytes.saturating_add(part.len().saturating_mul(2))
    })
}

impl StorageService {
    /// A fresh, short snapshot keeps graph membership, journal cursor and runtime facts coherent.
    /// It is observation only: all mutating repository entry points recheck their own authority.
    fn read_workflow_snapshot<T>(
        &self,
        read: impl FnOnce(&rusqlite::Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut connection = self.state.read_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let result = read(&transaction)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(result)
    }

    pub fn workflow_mail_receipt(
        &self,
        conversation_id: &str,
        run_id: &str,
        tool_call_id: &str,
        call: &crate::WorkflowMailReceiptCall,
    ) -> Result<Option<crate::WorkflowMailReceipt>, String> {
        repository::mail_receipt_for_call(
            &*self.state.read_connection()?,
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
            &*self.state.read_connection()?,
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
            &mut *self.state.read_connection()?,
        )
        .map_err(|error| error.to_string())?;
        let available_models: std::collections::HashSet<String> = stored
            .as_ref()
            .map(|stored| self.model_projection_from_stored(stored))
            .into_iter()
            .flat_map(|projection| projection.models)
            .filter(|entry| entry.execution.is_available())
            .map(|entry| entry.model.id)
            .collect();
        let estimated_bytes = estimated_json_bytes(request)?
            .saturating_add(estimated_json_bytes(&available_models)?)
            .saturating_add(512);
        // The credential coordinator is held by this caller. The transaction never takes it
        // or calls another StorageService, so the writer cannot wait back on its submitter.
        self.state
            .write_sync(estimated_bytes, StorageWriteClass::Normal, |connection| {
                crate::storage::organization_personnel_repository::manage(
                    connection,
                    request,
                    &available_models,
                )
            })?
    }
    pub fn workflow_execution_mutate(
        &self,
        request: &MutationRequest,
    ) -> Result<serde_json::Value, String> {
        self.state.write_sync(
            estimated_json_bytes(request)?.saturating_add(512),
            StorageWriteClass::Normal,
            |connection| repository::mutate(connection, request),
        )?
    }
    pub fn workflow_execution_settle_run(&self, run_id: &str, status: &str) -> Result<(), String> {
        self.state.write_sync(
            workflow_job_bytes(&[run_id, status]),
            StorageWriteClass::Normal,
            |connection| {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|e| e.to_string())?;
                repository::settle_run(&transaction, run_id, status)?;
                transaction.commit().map_err(|e| e.to_string())
            },
        )?
    }

    pub fn workflow_execution_awareness_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<serde_json::Value, String> {
        repository::awareness_for_conversation(&*self.state.read_connection()?, conversation_id)
    }
    pub fn workflow_execution_request_observation(
        &self,
        conversation_id: &str,
        run_id: Option<&str>,
    ) -> Result<Option<(ConversationSnapshot, serde_json::Value)>, String> {
        repository::request_observation(&*self.state.read_connection()?, conversation_id, run_id)
    }
    pub fn workflow_execution_state_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
        query: &crate::workflow_awareness::StateQuery,
    ) -> Result<serde_json::Value, String> {
        repository::state_for_run(
            &*self.state.read_connection()?,
            conversation_id,
            run_id,
            query,
        )
    }
    pub fn workflow_execution_mailbox_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
        query: &crate::workflow_awareness::MailboxQuery,
    ) -> Result<serde_json::Value, String> {
        repository::mailbox_for_run(
            &*self.state.read_connection()?,
            conversation_id,
            run_id,
            query,
        )
    }
    pub fn workflow_execution_awareness_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<serde_json::Value, String> {
        repository::awareness_for_run(&*self.state.read_connection()?, conversation_id, run_id)
    }
    pub fn workflow_execution_mark_run_unread(&self, run_id: &str) -> Result<(), String> {
        self.state.write_sync(
            workflow_job_bytes(&[run_id]),
            StorageWriteClass::Normal,
            |connection| repository::mark_run_unread(connection, run_id),
        )?
    }

    pub fn workflow_execution_snapshot_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Option<ConversationSnapshot>, String> {
        self.read_workflow_snapshot(|connection| {
            repository::snapshot_for_run(connection, conversation_id, run_id)
        })
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
        let mut connection = self.state.read_connection()?;
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
        repository::delivery_origins_for_conversation(
            &*self.state.read_connection()?,
            conversation_id,
        )
    }

    pub fn workflow_execution_recover_claims(&self) -> Result<(), String> {
        self.state.write_sync(
            workflow_job_bytes(&[]),
            StorageWriteClass::Normal,
            repository::recover_claims,
        )?
    }

    /// Completion recovery is scoped to the exact durable Run. Other Conversations may be
    /// preparing a claimed input whose initial Trace has not committed yet.
    pub fn workflow_execution_recover_run(&self, run_id: &str) -> Result<(), String> {
        self.state.write_sync(
            workflow_job_bytes(&[run_id]),
            StorageWriteClass::Normal,
            |connection| repository::recover_run(connection, run_id),
        )?
    }

    pub fn workflow_execution_bind_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Option<ConversationSnapshot>, String> {
        self.state.write_sync(
            workflow_job_bytes(&[conversation_id, run_id]),
            StorageWriteClass::Normal,
            |connection| repository::bind_run(connection, conversation_id, run_id),
        )?
    }
    pub fn workflow_execution_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationSnapshot>, String> {
        self.read_workflow_snapshot(|connection| {
            repository::snapshot_for_conversation(connection, conversation_id)
        })
    }
    pub fn workflow_execution_send(&self, request: &SendRequest) -> Result<SendReceipt, String> {
        self.state.write_sync(
            estimated_json_bytes(request)?.saturating_add(512),
            StorageWriteClass::Normal,
            |connection| repository::send(connection, request),
        )?
    }
    pub fn workflow_execution_pending_inputs(&self) -> Result<Vec<Input>, String> {
        self.read_workflow_snapshot(repository::pending_inputs)
    }
    pub fn workflow_execution_pending_candidates(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<PendingInputCandidate>, String> {
        repository::pending_candidates(&*self.state.read_connection()?, after_sequence, limit)
    }
    pub fn workflow_execution_eligible_pending_input(
        &self,
        input_id: &str,
    ) -> Result<Option<Input>, String> {
        self.read_workflow_snapshot(|connection| {
            repository::eligible_pending_input(connection, input_id)
        })
    }

    pub fn workflow_execution_bound_inputs(&self, run_id: &str) -> Result<Vec<Input>, String> {
        self.read_workflow_snapshot(|connection| repository::bound_inputs(connection, run_id))
    }
    pub fn workflow_execution_load_input(&self, input_id: &str) -> Result<Option<Input>, String> {
        repository::load_input(&*self.state.read_connection()?, input_id)
    }
    pub fn workflow_execution_inputs_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<Input>, String> {
        repository::inputs_for_conversation(&*self.state.read_connection()?, conversation_id)
    }
    pub fn workflow_execution_bind_input(
        &self,
        input_id: &str,
        run_id: &str,
        delivery_id: &str,
    ) -> Result<bool, String> {
        self.state.write_sync(
            workflow_job_bytes(&[input_id, run_id, delivery_id]),
            StorageWriteClass::Normal,
            |connection| repository::bind_input(connection, input_id, run_id, delivery_id),
        )?
    }
    pub fn workflow_execution_mark_applied(&self, input_id: &str) -> Result<(), String> {
        self.state.write_sync(
            workflow_job_bytes(&[input_id]),
            StorageWriteClass::Normal,
            |connection| repository::mark_applied(connection, input_id),
        )?
    }
    pub fn workflow_execution_fail_input(
        &self,
        input_id: &str,
        reason: &str,
    ) -> Result<(), String> {
        self.state.write_sync(
            workflow_job_bytes(&[input_id, reason]),
            StorageWriteClass::Normal,
            |connection| repository::fail_input(connection, input_id, reason),
        )?
    }
    pub fn workflow_execution_pause_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<(), String> {
        self.state.write_sync(
            workflow_job_bytes(&[conversation_id]),
            StorageWriteClass::Control,
            |connection| repository::pause_conversation(connection, conversation_id),
        )?
    }
    pub fn workflow_execution_resume_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<(), String> {
        self.state.write_sync(
            workflow_job_bytes(&[conversation_id]),
            StorageWriteClass::Control,
            |connection| repository::resume_conversation(connection, conversation_id),
        )?
    }
    pub fn workflow_execution_runtime(&self, instance_id: &str) -> Result<RuntimeSnapshot, String> {
        self.read_workflow_snapshot(|connection| {
            repository::runtime_snapshot(connection, instance_id, None)
        })
    }
    pub fn workflow_execution_runtime_since(
        &self,
        instance_id: &str,
        after_sequence: Option<u64>,
    ) -> Result<RuntimeSnapshot, String> {
        self.read_workflow_snapshot(|connection| {
            repository::runtime_snapshot(connection, instance_id, after_sequence)
        })
    }

    pub fn workflow_execution_runtime_summary(
        &self,
        instance_id: &str,
        after_sequence: Option<u64>,
    ) -> Result<RuntimeSnapshot, String> {
        let mut connection = self.state.read_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let snapshot = repository::runtime_summary(&transaction, instance_id, after_sequence)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(snapshot)
    }
}

#[cfg(test)]
#[path = "workflow_execution_read_tests.rs"]
mod read_pool_tests;
