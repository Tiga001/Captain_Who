use super::*;
use crate::storage::{ProviderTransitionCompatibleCommitOutcome, ProviderTransitionTerminalRecord};

impl StorageService {
    /// Cheap compatibility classification for an ordinary send, not a replay/integrity check.
    /// Reads only identities and projection metadata. The normal runtime still validates the
    /// journal and decrypts provider state before any model or tool execution.
    /// Uncertain staging, missing native tool projections, and released visible state require
    /// the existing full preflight (and, where necessary, an adaptation compaction).
    pub fn can_send_with_provider_protocol(
        &self,
        conversation_id: &str,
        protocol: &crate::ProviderProtocolKey,
    ) -> Result<bool, String> {
        let digest = crate::provider_continuation_store::protocol_digest(protocol)
            .map_err(|error| error.to_string())?;
        let native_replay = crate::resolve_provider_runtime_capabilities(protocol)
            .map_err(|error| error.to_string())?
            .private_replay()
            != crate::ProviderPrivateReplaySemantics::None;
        let connection = self.state.connection()?;
        // Include unactivated records conservatively: the full path owns crash recovery and may
        // promote a staged projection. A preliminary send check must not change durable state.
        let incompatible = connection
            .query_row(
                "SELECT EXISTS (
                SELECT 1 FROM provider_continuations p
                WHERE p.conversation_id = ?1 AND p.state IN ('active', 'superseded')
                  AND NOT EXISTS (
                    SELECT 1 FROM conversation_turn_rewrites r
                    WHERE r.conversation_id = p.conversation_id
                      AND r.source_assistant_message_id = p.assistant_message_id)
                  AND (?3 = 0 OR p.provider_protocol_digest != ?2 OR p.activated_at IS NULL)
            )",
                rusqlite::params![conversation_id, digest, native_replay],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_error)?;
        if incompatible {
            return Ok(false);
        }
        if !native_replay {
            return Ok(true);
        }
        // Compacted raw journals remain stored for exact recall. Only the visible suffix needs
        // native replay. A trace-item boundary retains the owning message's later narration and
        // final reply, whereas a message boundary covers the entire message.
        connection
            .query_row(
                "WITH boundary AS (
                SELECT m.position, s.covered_through_kind AS kind,
                       s.covered_through_trace_sequence AS sequence
                FROM conversation_context_compaction_heads h
                JOIN context_compaction_summaries s ON s.id = h.summary_id
                JOIN messages m ON m.id = s.covered_through_message_id
                WHERE h.conversation_id = ?1
            ), visible_tools AS (
                SELECT i.assistant_message_id, json_extract(i.item_json, '$.callId') AS call_id
                FROM conversation_turn_trace_items i
                JOIN messages m ON m.id = i.assistant_message_id
                WHERE m.conversation_id = ?1 AND i.item_kind = 'tool_call'
                  AND NOT EXISTS (SELECT 1 FROM conversation_turn_rewrites r
                    WHERE r.source_assistant_message_id = m.id AND r.conversation_id = ?1)
                  AND NOT EXISTS (SELECT 1 FROM boundary b WHERE m.position < b.position
                    OR (m.position = b.position AND
                      (b.kind = 'message' OR i.sequence <= b.sequence)))
            )
            SELECT NOT EXISTS (
                SELECT 1 FROM visible_tools t WHERE NOT EXISTS (
                    SELECT 1 FROM provider_continuation_tool_calls c
                    JOIN provider_continuations p ON p.continuation_id = c.continuation_id
                    WHERE p.conversation_id = ?1 AND p.assistant_message_id = t.assistant_message_id
                      AND c.runtime_call_id = t.call_id
                      AND p.state IN ('active', 'superseded') AND p.activated_at IS NOT NULL
                      AND p.provider_protocol_digest = ?2)
            ) AND NOT EXISTS (
                SELECT 1 FROM provider_continuations p
                JOIN messages m ON m.id = p.assistant_message_id
                WHERE p.conversation_id = ?1 AND p.state = 'released'
                  AND p.projection_kind IS NOT NULL
                  AND NOT EXISTS (SELECT 1 FROM conversation_turn_rewrites r
                    WHERE r.source_assistant_message_id = m.id AND r.conversation_id = ?1)
                  AND NOT EXISTS (SELECT 1 FROM boundary b WHERE m.position < b.position
                    OR (m.position = b.position AND (b.kind = 'message'
                      OR (p.projection_kind != 'conversation_message'
                          AND p.projection_sequence <= b.sequence))))
            )",
                rusqlite::params![conversation_id, digest],
                |row| row.get(0),
            )
            .map_err(storage_error)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_compatible_provider_transition_terminal(
        &self,
        operation_id: &str,
        conversation_id: &str,
        target_model_id: &str,
        source_model_display_name: Option<&str>,
        target_model_display_name: Option<&str>,
        started_at: i64,
        completed_at: i64,
        conversation_updated_at: i64,
        expected_current_model_id: Option<&str>,
        expected_conversation_updated_at: i64,
        expected_conversation_revision: i64,
        expected_target_provider_protocol_revision: &str,
    ) -> Result<ProviderTransitionCompatibleCommitOutcome, String> {
        let record = ProviderTransitionTerminalRecord::new(
            operation_id,
            conversation_id,
            target_model_id,
            source_model_display_name.map(str::to_string),
            target_model_display_name.map(str::to_string),
            started_at,
            completed_at,
            conversation_updated_at,
        )
        .map_err(|error| error.to_string())?;
        let mut connection = self.state.connection()?;
        provider_transition_repository::commit_compatible_transition(
            &mut connection,
            &record,
            expected_current_model_id,
            expected_conversation_updated_at,
            expected_conversation_revision,
            expected_target_provider_protocol_revision,
        )
        .map_err(|error| error.to_string())
    }

    pub fn get_provider_transition_terminal_record(
        &self,
        operation_id: &str,
    ) -> Result<Option<ProviderTransitionTerminalRecord>, String> {
        let connection = self.state.connection()?;
        provider_transition_repository::get_terminal_record(&connection, operation_id)
            .map_err(|error| error.to_string())
    }

    pub fn list_provider_transition_terminal_records(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<ProviderTransitionTerminalRecord>, String> {
        let connection = self.state.connection()?;
        provider_transition_repository::list_terminal_records(&connection, conversation_id, limit)
            .map_err(|error| error.to_string())
    }
}
