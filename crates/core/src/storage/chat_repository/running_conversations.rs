use crate::storage::models::RunningConversationSummary;
use rusqlite::Connection;

/// Trace headers are authoritative once admitted. Before admission, a pending assistant
/// placeholder keeps the queued conversation visible. No presentation JSON is reconstructed.
pub(crate) fn list_running_conversation_summaries(
    connection: &Connection,
) -> rusqlite::Result<Vec<RunningConversationSummary>> {
    let mut statement = connection.prepare(
        "SELECT conversation.id, conversation.title, conversation.updated_at
         FROM conversations AS conversation
         WHERE conversation.archived_at IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM agent_nodes AS node
               WHERE node.conversation_id = conversation.id
                 AND node.parent_agent_id IS NOT NULL
           )
           AND conversation.id IN (
               SELECT trace.conversation_id
               FROM conversation_turn_traces AS trace
               WHERE trace.terminal_status = 'in_progress'
                 AND NOT EXISTS (
                     SELECT 1 FROM conversation_turn_rewrites AS rewrite
                     WHERE rewrite.conversation_id = trace.conversation_id
                       AND rewrite.source_assistant_message_id = trace.assistant_message_id
                 )
               UNION
               SELECT message.conversation_id
               FROM messages AS message
               WHERE message.role = 'assistant' AND message.status = 'pending'
                 AND COALESCE(message.input_origin_kind, '') != 'agent'
                 AND NOT (COALESCE(message.input_origin_kind, '') = 'snapshot'
                          AND COALESCE(message.snapshot_original_origin_kind, '') = 'agent')
                 AND NOT EXISTS (
                     SELECT 1 FROM conversation_turn_traces AS trace
                     WHERE trace.assistant_message_id = message.id
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM conversation_turn_rewrites AS rewrite
                     WHERE rewrite.conversation_id = message.conversation_id
                       AND rewrite.source_assistant_message_id = message.id
                 )
           )
         ORDER BY conversation.updated_at DESC, conversation.id ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(RunningConversationSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?
        .collect();
    rows
}
