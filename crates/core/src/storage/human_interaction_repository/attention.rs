use super::*;

pub(super) const OPEN_REQUESTS: &str = "SELECT request.request_id, request.conversation_id, request.sequence, request.revision
    FROM human_interaction_requests AS request INDEXED BY idx_human_interaction_requests_attention
    CROSS JOIN conversations AS conversation ON conversation.id = request.conversation_id
    WHERE request.status = 'open' AND conversation.archived_at IS NULL
      AND NOT EXISTS (SELECT 1 FROM agent_nodes AS node WHERE node.conversation_id = conversation.id AND node.parent_agent_id IS NOT NULL)
    ORDER BY request.sequence";
pub(super) const OPEN_APPROVALS: &str = "SELECT DISTINCT action.conversation_id
    FROM agent_pending_actions AS action INDEXED BY idx_agent_pending_actions_status
    CROSS JOIN conversations AS conversation ON conversation.id = action.conversation_id
    WHERE action.status = 'pending' AND conversation.archived_at IS NULL
      AND NOT EXISTS (SELECT 1 FROM agent_nodes AS node WHERE node.conversation_id = conversation.id AND node.parent_agent_id IS NOT NULL)
    ORDER BY action.conversation_id";

/// One read transaction, three fixed metadata queries, independent of the watched chat count.
/// CROSS JOIN keeps sparse pending rows as the outer loop even without ANALYZE statistics;
/// otherwise SQLite can choose all unarchived conversations and scan each one's history.
/// AUTOINCREMENT's high-water mark survives deletion, so a missing older open notification
/// cannot resurrect a settled/deleted request after an authoritative snapshot.
pub fn load_attention(connection: &mut Connection) -> Result<HumanInteractionAttentionSnapshot> {
    let tx = connection.transaction().map_err(unavailable)?;
    let request_sequence = tx.query_row(
        "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='human_interaction_requests'), 0)",
        [], |row| row.get(0),
    ).map_err(unavailable)?;
    let requests = tx
        .prepare(OPEN_REQUESTS)
        .map_err(unavailable)?
        .query_map([], |row| {
            Ok(HumanInteractionAttentionRequest {
                request_id: row.get(0)?,
                conversation_id: row.get(1)?,
                sequence: row.get(2)?,
                revision: row.get(3)?,
            })
        })
        .map_err(unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(unavailable)?;
    let approval_conversation_ids = tx
        .prepare(OPEN_APPROVALS)
        .map_err(unavailable)?
        .query_map([], |row| row.get(0))
        .map_err(unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(unavailable)?;
    tx.commit().map_err(unavailable)?;
    Ok(HumanInteractionAttentionSnapshot {
        request_sequence,
        requests,
        approval_conversation_ids,
    })
}
