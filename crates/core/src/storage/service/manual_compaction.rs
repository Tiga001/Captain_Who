use super::*;
use crate::storage::manual_context_compaction_repository as repository;
use crate::storage::models::{
    ManualContextCompactionOperation, ManualContextCompactionUsageRecord,
};

impl StorageService {
    pub fn record_manual_context_compaction_observation_and_usage(
        &self,
        observation: &ModelRequestObservation,
        record: &ManualContextCompactionUsageRecord,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        if observation.operation_id.as_deref() != Some(record.operation_id.as_str())
            || observation.conversation_id.as_deref() != Some(record.conversation_id.as_str())
        {
            return Err("手动压缩用量与模型观测归属不一致。".to_string());
        }
        model_request_observation_repository::insert_observation(&transaction, observation)
            .map_err(|error| error.to_string())?;
        repository::record_usage(&transaction, record)?;
        transaction.commit().map_err(storage_error)
    }
    pub fn claim_manual_context_compaction(
        &self,
        operation: &ManualContextCompactionOperation,
    ) -> Result<ManualContextCompactionOperation, String> {
        repository::claim(&mut *self.state.connection()?, operation)
    }
    pub fn get_manual_context_compaction(
        &self,
        conversation_id: &str,
        operation_id: Option<&str>,
    ) -> Result<Option<ManualContextCompactionOperation>, String> {
        repository::get(&*self.state.connection()?, conversation_id, operation_id)
    }
    pub fn list_manual_context_compactions(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ManualContextCompactionOperation>, String> {
        repository::list(&*self.state.connection()?, conversation_id)
    }
    pub fn update_manual_context_compaction(
        &self,
        operation: &ManualContextCompactionOperation,
    ) -> Result<ManualContextCompactionOperation, String> {
        repository::update(&*self.state.connection()?, operation)
    }
    pub fn record_manual_context_compaction_usage(
        &self,
        record: &ManualContextCompactionUsageRecord,
    ) -> Result<(), String> {
        repository::record_usage(&*self.state.connection()?, record)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn commit_manual_context_compaction_if_current(
        &self,
        prefix: &ContextCompactionPrefix,
        draft: ContextCompactionSummaryDraft,
        receipt: &ContextCompactionReceipt,
        observation: &ModelRequestObservation,
        operation: &ManualContextCompactionOperation,
        expected_model_id: &str,
        expected_provider_protocol_revision: &str,
    ) -> Result<Option<ContextCompactionSummary>, String> {
        repository::commit_if_current(
            &mut *self.state.connection()?,
            prefix,
            draft,
            receipt,
            observation,
            operation,
            expected_model_id,
            expected_provider_protocol_revision,
        )
    }
}
