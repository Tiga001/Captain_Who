use super::*;

impl StorageService {
    /// Reconciles crash-only ordinary Provider continuation staging at Host startup.
    ///
    /// The detailed row counts remain repository-private; callers only need the fail-closed
    /// success boundary before any generic orphan-trace retirement runs.
    pub fn reconcile_staged_provider_continuations_on_startup(
        &self,
        reconciled_at: i64,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        provider_continuation_repository::reconcile_staged_projections_at_startup(
            &mut connection,
            reconciled_at,
        )
        .map(|_| ())
        .map_err(storage_error)
    }

    pub(crate) fn store_staged_provider_continuation(
        &self,
        record: &provider_continuation_repository::ProviderContinuationEnvelopeRecord,
    ) -> Result<provider_continuation_repository::ProviderContinuationStoreOutcome, String> {
        let mut connection = self.state.connection()?;
        provider_continuation_repository::store_staged(&mut connection, record)
            .map_err(storage_error)
    }

    pub(crate) fn store_staged_provider_continuation_with_projection(
        &self,
        record: &provider_continuation_repository::ProviderContinuationEnvelopeRecord,
        projection: provider_continuation_repository::ProviderContinuationProjection,
    ) -> Result<provider_continuation_repository::ProviderContinuationStoreOutcome, String> {
        let mut connection = self.state.connection()?;
        provider_continuation_repository::store_staged_with_projection(
            &mut connection,
            record,
            projection,
        )
        .map_err(storage_error)
    }

    pub(crate) fn promote_staged_provider_continuation(
        &self,
        continuation_id: &str,
        promoted_at: i64,
    ) -> Result<provider_continuation_repository::ProviderContinuationPromotionOutcome, String>
    {
        let mut connection = self.state.connection()?;
        provider_continuation_repository::promote_staged(
            &mut connection,
            continuation_id,
            promoted_at,
        )
        .map_err(storage_error)
    }

    #[allow(dead_code)] // Exact-ref Vault API; normal hydration uses conversation-wide loading.
    pub(crate) fn load_provider_continuation(
        &self,
        continuation_id: &str,
    ) -> Result<Option<provider_continuation_repository::StoredProviderContinuationRecord>, String>
    {
        let connection = self.state.connection()?;
        provider_continuation_repository::load(&connection, continuation_id).map_err(storage_error)
    }

    pub(crate) fn list_replayable_provider_continuations_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<provider_continuation_repository::StoredProviderContinuationRecord>, String>
    {
        let connection = self.state.connection()?;
        provider_continuation_repository::list_replayable_for_conversation(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)
    }

    pub(crate) fn has_replayable_provider_continuations_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<bool, String> {
        let connection = self.state.connection()?;
        provider_continuation_repository::has_replayable_for_conversation(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)
    }

    pub(crate) fn has_released_provider_continuations_for_projections(
        &self,
        conversation_id: &str,
        projections: &[(
            String,
            provider_continuation_repository::ProviderContinuationProjection,
        )],
    ) -> Result<bool, String> {
        let connection = self.state.connection()?;
        let cursors = projections
            .iter()
            .map(|(assistant_message_id, projection)| match projection {
                provider_continuation_repository::ProviderContinuationProjection::ConversationMessage => {
                    provider_continuation_repository::ProviderContinuationProjectionCursor::ConversationMessage {
                        assistant_message_id: assistant_message_id.clone(),
                    }
                }
                provider_continuation_repository::ProviderContinuationProjection::ConversationTraceItem {
                    sequence,
                    ..
                } => {
                    provider_continuation_repository::ProviderContinuationProjectionCursor::ConversationTraceItem {
                        assistant_message_id: assistant_message_id.clone(),
                        sequence: *sequence,
                    }
                }
                provider_continuation_repository::ProviderContinuationProjection::ConversationSteerBoundary {
                    guidance_sequence,
                } => {
                    provider_continuation_repository::ProviderContinuationProjectionCursor::ConversationTraceItem {
                        assistant_message_id: assistant_message_id.clone(),
                        sequence: *guidance_sequence,
                    }
                }
            })
            .collect::<Vec<_>>();
        provider_continuation_repository::has_released_for_covered_projections(
            &connection,
            conversation_id,
            &cursors,
        )
        .map_err(storage_error)
    }

    #[allow(dead_code)] // Exact-ref Vault API; transactional lifecycle paths bypass this wrapper.
    pub(crate) fn release_provider_continuation(
        &self,
        continuation_id: &str,
        released_at: i64,
    ) -> Result<provider_continuation_repository::ProviderContinuationReleaseOutcome, String> {
        let connection = self.state.connection()?;
        provider_continuation_repository::release(&connection, continuation_id, released_at)
            .map_err(storage_error)
    }
}
