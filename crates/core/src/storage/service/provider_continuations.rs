use super::*;

impl StorageService {
    pub(crate) fn store_staged_provider_continuation(
        &self,
        record: &provider_continuation_repository::ProviderContinuationEnvelopeRecord,
    ) -> Result<provider_continuation_repository::ProviderContinuationStoreOutcome, String> {
        let mut connection = self.state.connection()?;
        provider_continuation_repository::store_staged(&mut connection, record)
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
