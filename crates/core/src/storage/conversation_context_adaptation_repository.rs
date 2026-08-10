use rusqlite::{params, Connection, OptionalExtension};

pub(crate) const CONVERSATION_CONTEXT_ADAPTATION_SCHEMA_VERSION: u32 = 1;
pub(crate) const FORK_RELEASED_PROVIDER_STATE_REASON: &str =
    "fork_released_provider_state_requires_compaction";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConversationContextAdaptationRequirement {
    pub(crate) conversation_id: String,
    pub(crate) reason: String,
    pub(crate) source_conversation_id: String,
    pub(crate) source_message_id: String,
    pub(crate) created_at: i64,
    pub(crate) resolved_summary_id: Option<String>,
    pub(crate) resolved_at: Option<i64>,
}

impl ConversationContextAdaptationRequirement {
    pub(crate) fn is_required(&self) -> bool {
        self.resolved_summary_id.is_none()
    }
}

pub(crate) fn get(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<ConversationContextAdaptationRequirement>> {
    connection
        .query_row(
            "SELECT schema_version, reason, source_conversation_id, source_message_id, created_at,
                    resolved_summary_id, resolved_at
             FROM conversation_context_adaptation_requirements
             WHERE conversation_id = ?1",
            [conversation_id],
            |row| {
                let schema_version = row.get::<_, u32>(0)?;
                let reason = row.get::<_, String>(1)?;
                let source_conversation_id = row.get::<_, String>(2)?;
                let source_message_id = row.get::<_, String>(3)?;
                let created_at = row.get::<_, i64>(4)?;
                let resolved_summary_id = row.get::<_, Option<String>>(5)?;
                let resolved_at = row.get::<_, Option<i64>>(6)?;
                if schema_version != CONVERSATION_CONTEXT_ADAPTATION_SCHEMA_VERSION
                    || reason != FORK_RELEASED_PROVIDER_STATE_REASON
                    || source_conversation_id.trim().is_empty()
                    || source_message_id.trim().is_empty()
                    || created_at < 0
                    || (resolved_summary_id.is_some() != resolved_at.is_some())
                    || resolved_summary_id
                        .as_deref()
                        .is_some_and(|summary_id| summary_id.trim().is_empty())
                    || resolved_at.is_some_and(|resolved_at| resolved_at < created_at)
                {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(ConversationContextAdaptationRequirement {
                    conversation_id: conversation_id.to_string(),
                    reason,
                    source_conversation_id,
                    source_message_id,
                    created_at,
                    resolved_summary_id,
                    resolved_at,
                })
            },
        )
        .optional()
}

pub(crate) fn insert_in_connection(
    connection: &Connection,
    requirement: &ConversationContextAdaptationRequirement,
) -> rusqlite::Result<()> {
    if requirement.conversation_id.trim().is_empty()
        || requirement.reason != FORK_RELEASED_PROVIDER_STATE_REASON
        || requirement.source_conversation_id.trim().is_empty()
        || requirement.source_message_id.trim().is_empty()
        || requirement.created_at < 0
        || (requirement.resolved_summary_id.is_some() != requirement.resolved_at.is_some())
        || requirement
            .resolved_summary_id
            .as_deref()
            .is_some_and(|summary_id| summary_id.trim().is_empty())
        || requirement
            .resolved_at
            .is_some_and(|resolved_at| resolved_at < requirement.created_at)
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid conversation context adaptation requirement".to_string(),
        ));
    }
    let inserted = connection.execute(
        "INSERT INTO conversation_context_adaptation_requirements (
            conversation_id, schema_version, reason, source_conversation_id,
            source_message_id, created_at, resolved_summary_id, resolved_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &requirement.conversation_id,
            CONVERSATION_CONTEXT_ADAPTATION_SCHEMA_VERSION,
            &requirement.reason,
            &requirement.source_conversation_id,
            &requirement.source_message_id,
            requirement.created_at,
            &requirement.resolved_summary_id,
            requirement.resolved_at,
        ],
    )?;
    if inserted != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

pub(crate) fn resolve_in_connection(
    connection: &Connection,
    conversation_id: &str,
    summary_id: &str,
    resolved_at: i64,
) -> rusqlite::Result<bool> {
    if summary_id.trim().is_empty() || resolved_at < 0 {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid resolved conversation context adaptation boundary".to_string(),
        ));
    }
    Ok(connection.execute(
        "UPDATE conversation_context_adaptation_requirements
         SET resolved_summary_id = ?1, resolved_at = ?2
         WHERE conversation_id = ?3
           AND resolved_summary_id IS NULL
           AND resolved_at IS NULL",
        params![summary_id, resolved_at, conversation_id],
    )? == 1)
}
