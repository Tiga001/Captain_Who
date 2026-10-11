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
        self.state.write_sync(
            1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                provider_continuation_repository::reconcile_staged_projections_at_startup(
                    connection,
                    reconciled_at,
                )
                .map(|_| ())
                .map_err(storage_error)
            },
        )?
    }

    pub(crate) fn store_staged_provider_continuation(
        &self,
        record: &provider_continuation_repository::ProviderContinuationEnvelopeRecord,
    ) -> Result<provider_continuation_repository::ProviderContinuationStoreOutcome, String> {
        self.state.write_sync(
            provider_envelope_job_bytes(record),
            crate::storage::StorageWriteClass::Control,
            |connection| {
                provider_continuation_repository::store_staged(connection, record)
                    .map_err(storage_error)
            },
        )?
    }

    pub(crate) fn store_staged_provider_continuation_with_projection(
        &self,
        record: &provider_continuation_repository::ProviderContinuationEnvelopeRecord,
        projection: provider_continuation_repository::ProviderContinuationProjection,
    ) -> Result<provider_continuation_repository::ProviderContinuationStoreOutcome, String> {
        self.state.write_sync(
            provider_envelope_job_bytes(record),
            crate::storage::StorageWriteClass::Control,
            |connection| {
                provider_continuation_repository::store_staged_with_projection(
                    connection, record, projection,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub(crate) fn promote_staged_provider_continuation(
        &self,
        continuation_id: &str,
        promoted_at: i64,
    ) -> Result<provider_continuation_repository::ProviderContinuationPromotionOutcome, String>
    {
        self.state.write_sync(
            continuation_id.len() + 1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                provider_continuation_repository::promote_staged(
                    connection,
                    continuation_id,
                    promoted_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    #[allow(dead_code)] // Exact-ref integrity lookup; normal hydration uses conversation-wide loading.
    pub(crate) fn load_provider_continuation(
        &self,
        continuation_id: &str,
    ) -> Result<Option<provider_continuation_repository::StoredProviderContinuationRecord>, String>
    {
        let connection = self.state.read_connection()?;
        let connection = connection.unchecked_transaction().map_err(storage_error)?;
        provider_continuation_repository::load(&connection, continuation_id).map_err(storage_error)
    }

    pub(crate) fn list_replayable_provider_continuations_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<provider_continuation_repository::StoredProviderContinuationRecord>, String>
    {
        // These lookups also recover durable-but-unpromoted envelopes. Keep their writes and
        // result in one writer transaction; a query_only reader must never perform promotion.
        self.state.write_sync(
            conversation_id.len().saturating_add(256),
            crate::storage::StorageWriteClass::Normal,
            |connection| {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(storage_error)?;
                let result = provider_continuation_repository::list_replayable_for_conversation(
                    &transaction,
                    conversation_id,
                )
                .map_err(storage_error)?;
                transaction.commit().map_err(storage_error)?;
                Ok(result)
            },
        )?
    }

    pub(crate) fn has_replayable_provider_continuations_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<bool, String> {
        // These lookups also recover durable-but-unpromoted envelopes. Keep their writes and
        // result in one writer transaction; a query_only reader must never perform promotion.
        self.state.write_sync(
            conversation_id.len().saturating_add(256),
            crate::storage::StorageWriteClass::Normal,
            |connection| {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(storage_error)?;
                let result = provider_continuation_repository::has_replayable_for_conversation(
                    &transaction,
                    conversation_id,
                )
                .map_err(storage_error)?;
                transaction.commit().map_err(storage_error)?;
                Ok(result)
            },
        )?
    }

    pub(crate) fn has_released_provider_continuations_for_projections(
        &self,
        conversation_id: &str,
        projections: &[(
            String,
            provider_continuation_repository::ProviderContinuationProjection,
        )],
    ) -> Result<bool, String> {
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
        let connection = self.state.read_connection()?;
        let connection = connection.unchecked_transaction().map_err(storage_error)?;
        provider_continuation_repository::has_released_for_covered_projections(
            &connection,
            conversation_id,
            &cursors,
        )
        .map_err(storage_error)
    }

    pub(crate) fn release_provider_continuation(
        &self,
        continuation_id: &str,
        released_at: i64,
    ) -> Result<provider_continuation_repository::ProviderContinuationReleaseOutcome, String> {
        self.state.write_sync(
            continuation_id.len() + 1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                provider_continuation_repository::release(connection, continuation_id, released_at)
                    .map_err(storage_error)
            },
        )?
    }
}

fn provider_envelope_job_bytes(
    record: &provider_continuation_repository::ProviderContinuationEnvelopeRecord,
) -> usize {
    record.continuation_id.len()
        + record.conversation_id.len()
        + record.assistant_message_id.len()
        + record.run_id.len()
        + record.assistant_turn_id.len()
        + record.assistant_turn_digest.len()
        + record.provider_protocol_digest.len()
        + record.payload_digest.len()
        + record.nonce.len()
        + record.ciphertext.len()
        + record
            .runtime_tool_calls
            .iter()
            .map(|c| c.runtime_call_id.len() + 64)
            .sum::<usize>()
        + 1024
}
