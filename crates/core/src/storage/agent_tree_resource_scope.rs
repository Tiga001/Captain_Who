use rusqlite::{Connection, OptionalExtension};

/// Trusted storage identity used only to authorize private inputs inside one Agent task tree.
///
/// Callers must resolve this from `agent_nodes`; model-, Renderer-, or RPC-provided root IDs are
/// never accepted as authority. Ordinary conversations deliberately have no tree scope. Agent
/// lifecycle is not part of this identity: a root must still be able to consume an archived or
/// completed child's durable result, while both immutable root IDs continue to enforce isolation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentTreeResourceScope {
    pub(crate) root_agent_id: String,
    pub(crate) root_conversation_id: String,
}

pub(crate) fn for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<AgentTreeResourceScope>> {
    connection
        .query_row(
            "SELECT root_agent_id, root_conversation_id
             FROM agent_nodes
             WHERE conversation_id = ?1",
            [conversation_id],
            |row| {
                Ok(AgentTreeResourceScope {
                    root_agent_id: row.get(0)?,
                    root_conversation_id: row.get(1)?,
                })
            },
        )
        .optional()
}

pub(crate) fn conversations_share_tree(
    connection: &Connection,
    left_conversation_id: &str,
    right_conversation_id: &str,
) -> rusqlite::Result<bool> {
    let Some(left) = for_conversation(connection, left_conversation_id)? else {
        return Ok(false);
    };
    let Some(right) = for_conversation(connection, right_conversation_id)? else {
        return Ok(false);
    };
    Ok(left == right)
}
