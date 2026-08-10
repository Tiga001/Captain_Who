use super::*;

impl StorageService {
    pub fn conversation_requires_context_adaptation(
        &self,
        conversation_id: &str,
    ) -> Result<bool, String> {
        let connection = self.state.connection()?;
        conversation_context_adaptation_repository::get(&connection, conversation_id)
            .map(|requirement| requirement.is_some_and(|requirement| requirement.is_required()))
            .map_err(storage_error)
    }

    pub fn get_active_context_compaction_summary(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ContextCompactionSummary>, String> {
        let connection = self.state.connection()?;
        context_compaction_repository::get_active_summary(&connection, conversation_id)
            .map_err(|error| error.to_string())
    }

    pub fn save_model_request_observation(
        &self,
        observation: &ModelRequestObservation,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        model_request_observation_repository::insert_observation(&connection, observation)
            .map_err(|error| error.to_string())
    }

    pub fn record_context_compaction_receipt(
        &self,
        receipt: &ContextCompactionReceipt,
        observation: Option<&ModelRequestObservation>,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        context_compaction_receipt_repository::record_receipt(&mut connection, receipt, observation)
            .map_err(|error| error.to_string())
    }

    pub fn list_provider_transition_receipts(
        &self,
        conversation_id: &str,
        operation_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ContextCompactionReceipt>, String> {
        let connection = self.state.connection()?;
        context_compaction_receipt_repository::list_provider_transition_receipts(
            &connection,
            conversation_id,
            operation_id,
            limit,
        )
        .map_err(|error| error.to_string())
    }

    pub fn provider_transition_failed_attempt_count(
        &self,
        conversation_id: &str,
        target_model_id: &str,
    ) -> Result<u64, String> {
        let connection = self.state.connection()?;
        context_compaction_receipt_repository::provider_transition_failed_attempt_count(
            &connection,
            conversation_id,
            target_model_id,
        )
        .map_err(|error| error.to_string())
    }

    pub fn prepare_context_compaction_prefix(
        &self,
        conversation_id: &str,
        covered_through: &ContextJournalCursor,
    ) -> Result<ContextCompactionPrefix, String> {
        let connection = self.state.connection()?;
        context_compaction_repository::prepare_prefix(&connection, conversation_id, covered_through)
            .map_err(|error| error.to_string())
    }

    /// Returns `None` when the planner's active-summary identity is stale. The caller should
    /// refresh its durable baseline and plan again instead of treating this as a failed run.
    pub fn prepare_context_compaction_prefix_if_current(
        &self,
        conversation_id: &str,
        covered_through: &ContextJournalCursor,
        expected_active_summary_id: Option<&str>,
    ) -> Result<Option<ContextCompactionPrefix>, String> {
        let connection = self.state.connection()?;
        let active =
            match context_compaction_repository::get_active_summary(&connection, conversation_id) {
                Ok(active) => active,
                Err(error) if error.is_stale() => return Ok(None),
                Err(error) => return Err(error.to_string()),
            };
        if active.as_ref().map(|summary| summary.id.as_str()) != expected_active_summary_id {
            return Ok(None);
        }
        match context_compaction_repository::prepare_prefix(
            &connection,
            conversation_id,
            covered_through,
        ) {
            Ok(prefix) => Ok(Some(prefix)),
            Err(error) if error.is_stale() => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn commit_context_compaction_prefix(
        &self,
        expected_prefix: &ContextCompactionPrefix,
        draft: ContextCompactionSummaryDraft,
        introduced_by_assistant_message_id: &str,
    ) -> Result<ContextCompactionSummary, String> {
        let mut connection = self.state.connection()?;
        context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            expected_prefix,
            draft,
            introduced_by_assistant_message_id,
        )
        .map_err(|error| error.to_string())
    }

    /// Commits only if the prepared durable prefix is still current. `None` is a normal stale
    /// outcome and must cause a baseline refresh plus replanning.
    pub fn commit_context_compaction_prefix_if_current(
        &self,
        expected_prefix: &ContextCompactionPrefix,
        draft: ContextCompactionSummaryDraft,
        introduced_by_assistant_message_id: &str,
    ) -> Result<Option<ContextCompactionSummary>, String> {
        let mut connection = self.state.connection()?;
        match context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            expected_prefix,
            draft,
            introduced_by_assistant_message_id,
        ) {
            Ok(summary) => Ok(Some(summary)),
            Err(error) if error.is_stale() => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Atomically commits a successful compaction and its diagnostic evidence. `None` means the
    /// prepared prefix became stale; the transaction leaves no observation, summary, head or
    /// terminal receipt behind in that case.
    pub fn commit_context_compaction_prefix_with_receipt_if_current(
        &self,
        expected_prefix: &ContextCompactionPrefix,
        draft: ContextCompactionSummaryDraft,
        receipt: &ContextCompactionReceipt,
        observation: &ModelRequestObservation,
    ) -> Result<Option<ContextCompactionSummary>, String> {
        let mut connection = self.state.connection()?;
        match context_compaction_repository::commit_prefix_replacement_with_receipt(
            &mut connection,
            expected_prefix,
            draft,
            receipt,
            observation,
        ) {
            Ok(summary) => Ok(Some(summary)),
            Err(error) if error.is_stale() => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_provider_transition_with_receipt_if_current(
        &self,
        expected_prefix: &ContextCompactionPrefix,
        draft: ContextCompactionSummaryDraft,
        receipt: &ContextCompactionReceipt,
        observation: &ModelRequestObservation,
        expected_current_model_id: Option<&str>,
        expected_conversation_updated_at: i64,
        target_model_id: &str,
        expected_target_provider_protocol_revision: &str,
    ) -> Result<Option<(ContextCompactionSummary, i64)>, String> {
        let mut connection = self.state.connection()?;
        match context_compaction_repository::commit_provider_transition_with_receipt(
            &mut connection,
            expected_prefix,
            draft,
            receipt,
            observation,
            expected_current_model_id,
            expected_conversation_updated_at,
            target_model_id,
            expected_target_provider_protocol_revision,
        ) {
            Ok(committed) => Ok(Some(committed)),
            Err(error) if error.is_stale() => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn rollback_context_compaction_summary(
        &self,
        conversation_id: &str,
        expected_summary_id: &str,
        updated_at: i64,
    ) -> Result<Option<ContextCompactionSummary>, String> {
        let mut connection = self.state.connection()?;
        context_compaction_repository::rollback_active_summary(
            &mut connection,
            conversation_id,
            expected_summary_id,
            updated_at,
        )
        .map_err(|error| error.to_string())
    }
}
