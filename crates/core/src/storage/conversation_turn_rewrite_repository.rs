use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTurnRewriteAdmission {
    pub request_id: String,
    pub request_fingerprint: String,
    pub conversation_id: String,
    pub source_user_message_id: String,
    pub source_assistant_message_id: String,
    pub replacement_user_message_id: String,
    pub replacement_assistant_message_id: String,
    pub run_id: String,
    pub response_json: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTurnRewriteRecord {
    pub request_id: String,
    pub request_fingerprint: String,
    pub conversation_id: String,
    pub source_user_message_id: String,
    pub source_assistant_message_id: String,
    pub replacement_user_message_id: String,
    pub replacement_assistant_message_id: String,
    pub run_id: String,
    pub response_json: String,
    pub created_at: i64,
}

pub fn get_by_request_id(
    connection: &Connection,
    request_id: &str,
) -> rusqlite::Result<Option<ConversationTurnRewriteRecord>> {
    connection
        .query_row(
            "SELECT request_id, request_fingerprint, conversation_id,
                    source_user_message_id, source_assistant_message_id,
                    replacement_user_message_id, replacement_assistant_message_id,
                    run_id, response_json, created_at
             FROM conversation_turn_rewrites
             WHERE request_id = ?1",
            [request_id],
            |row| {
                Ok(ConversationTurnRewriteRecord {
                    request_id: row.get(0)?,
                    request_fingerprint: row.get(1)?,
                    conversation_id: row.get(2)?,
                    source_user_message_id: row.get(3)?,
                    source_assistant_message_id: row.get(4)?,
                    replacement_user_message_id: row.get(5)?,
                    replacement_assistant_message_id: row.get(6)?,
                    run_id: row.get(7)?,
                    response_json: row.get(8)?,
                    created_at: row.get(9)?,
                })
            },
        )
        .optional()
}

pub(crate) fn validate_source_is_editable_tail(
    connection: &Connection,
    admission: &ConversationTurnRewriteAdmission,
) -> Result<(), String> {
    for (field, value) in [
        ("requestId", admission.request_id.as_str()),
        ("conversationId", admission.conversation_id.as_str()),
        (
            "sourceUserMessageId",
            admission.source_user_message_id.as_str(),
        ),
        (
            "sourceAssistantMessageId",
            admission.source_assistant_message_id.as_str(),
        ),
        (
            "replacementUserMessageId",
            admission.replacement_user_message_id.as_str(),
        ),
        (
            "replacementAssistantMessageId",
            admission.replacement_assistant_message_id.as_str(),
        ),
        ("runId", admission.run_id.as_str()),
    ] {
        if value.trim().is_empty() || value.len() > 256 {
            return Err(format!("Conversation Turn rewrite {field} is invalid"));
        }
    }
    if admission.created_at < 0
        || admission.source_user_message_id == admission.source_assistant_message_id
        || admission.replacement_user_message_id == admission.replacement_assistant_message_id
        || admission.source_user_message_id == admission.replacement_user_message_id
        || admission.source_assistant_message_id == admission.replacement_assistant_message_id
    {
        return Err("Conversation Turn rewrite identity is invalid".to_string());
    }

    let source = connection
        .query_row(
            "SELECT source_user.position, source_assistant.position,
                    source_trace.terminal_status
             FROM messages AS source_user
             JOIN messages AS source_assistant
               ON source_assistant.id = ?3
              AND source_assistant.conversation_id = source_user.conversation_id
             JOIN conversation_turn_traces AS source_trace
               ON source_trace.assistant_message_id = source_assistant.id
              AND source_trace.conversation_id = source_assistant.conversation_id
             WHERE source_user.id = ?2
               AND source_user.conversation_id = ?1
               AND source_user.role = 'user'
               AND source_assistant.role = 'assistant'
               AND source_user.status = 'sent'
               AND (
                    source_user.input_origin_kind IS NULL
                    OR source_user.input_origin_kind = 'human'
                    OR (
                        source_user.input_origin_kind = 'snapshot'
                        AND source_user.snapshot_original_origin_kind = 'human'
                    )
               )
               AND NOT EXISTS (
                    SELECT 1 FROM conversation_turn_rewrites AS hidden
                    WHERE hidden.conversation_id = ?1
                      AND (
                        hidden.source_user_message_id = source_user.id
                        OR hidden.source_assistant_message_id = source_assistant.id
                      )
               )",
            params![
                &admission.conversation_id,
                &admission.source_user_message_id,
                &admission.source_assistant_message_id,
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            "edit_turn_not_editable: source must be one visible human/assistant Turn".to_string()
        })?;
    if source.0 >= source.1 || !matches!(source.2.as_str(), "completed" | "failed" | "cancelled") {
        return Err(
            "edit_turn_not_settled: only a completed, failed, or interrupted Turn can be edited"
                .to_string(),
        );
    }

    let has_later_user_facing_turn = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM messages AS later
                WHERE later.conversation_id = ?1
                  AND later.position > ?2
                  AND NOT EXISTS (
                      SELECT 1 FROM conversation_turn_rewrites AS hidden
                      WHERE hidden.conversation_id = later.conversation_id
                        AND (
                            hidden.source_user_message_id = later.id
                            OR hidden.source_assistant_message_id = later.id
                        )
                  )
                  AND (
                      later.role = 'assistant'
                      OR (
                          later.role = 'user'
                          AND (
                              later.input_origin_kind IS NULL
                              OR later.input_origin_kind = 'human'
                              OR (
                                  later.input_origin_kind = 'snapshot'
                                  AND later.snapshot_original_origin_kind = 'human'
                              )
                          )
                      )
                  )
            )",
            params![&admission.conversation_id, source.1],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if has_later_user_facing_turn {
        return Err(
            "edit_turn_not_latest: only the latest visible human Turn can be edited".to_string(),
        );
    }

    let active_run = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM conversation_turn_traces
                WHERE conversation_id = ?1 AND terminal_status = 'in_progress'
            )",
            [&admission.conversation_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if active_run {
        return Err("edit_turn_active_run: wait for the active Turn to finish".to_string());
    }

    let pending_approval = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM agent_pending_actions
                WHERE conversation_id = ?1
                  AND status IN ('pending', 'approved', 'executing')
            )",
            [&admission.conversation_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if pending_approval {
        return Err(
            "edit_turn_pending_approval: settle the pending approval before editing".to_string(),
        );
    }

    let active_command_session = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM agent_command_sessions
                WHERE conversation_id = ?1
                  AND status IN ('starting', 'running')
            )",
            [&admission.conversation_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if active_command_session {
        return Err(
            "edit_turn_command_session_busy: wait for active Command Sessions to settle"
                .to_string(),
        );
    }

    let collaboration_busy = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM agent_nodes AS root
                JOIN agent_wake_requests AS wake ON wake.root_agent_id = root.root_agent_id
                WHERE root.conversation_id = ?1
                  AND wake.agent_id != root.agent_id
                  AND wake.status IN ('queued', 'claimed', 'running', 'waiting_for_approval')
                UNION ALL
                SELECT 1
                FROM agent_nodes AS root
                JOIN agent_mailbox_messages AS mailbox
                  ON mailbox.root_agent_id = root.root_agent_id
                WHERE root.conversation_id = ?1
                  AND (
                      mailbox.delivery_status IN ('queued', 'claimed')
                      OR (
                          mailbox.kind = 'result'
                          AND NOT EXISTS (
                              SELECT 1
                              FROM agent_model_batch_receipt_items AS consumed
                              WHERE consumed.message_id = mailbox.message_id
                          )
                      )
                  )
            )",
            [&admission.conversation_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if collaboration_busy {
        return Err(
            "edit_turn_collaboration_busy: wait for child Agents and pending results to settle"
                .to_string(),
        );
    }

    let summary_covers_source = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM conversation_context_compaction_heads AS head
                JOIN context_compaction_summaries AS summary ON summary.id = head.summary_id
                JOIN messages AS boundary ON boundary.id = summary.covered_through_message_id
                WHERE head.conversation_id = ?1
                  AND summary.conversation_id = ?1
                  AND boundary.conversation_id = ?1
                  AND boundary.position >= ?2
            )",
            params![&admission.conversation_id, source.0],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if summary_covers_source {
        return Err(
            "edit_turn_compacted: the latest Turn is already covered by the active context summary"
                .to_string(),
        );
    }
    let source_owns_active_summary = connection
        .query_row(
            "WITH RECURSIVE active_summary_chain(id, previous_summary_id) AS (
                 SELECT summary.id, summary.previous_summary_id
                 FROM conversation_context_compaction_heads AS head
                 JOIN context_compaction_summaries AS summary ON summary.id = head.summary_id
                 WHERE head.conversation_id = ?1
                 UNION ALL
                 SELECT previous.id, previous.previous_summary_id
                 FROM context_compaction_summaries AS previous
                 JOIN active_summary_chain AS chain ON previous.id = chain.previous_summary_id
                 WHERE previous.conversation_id = ?1
             )
             SELECT EXISTS(
                 SELECT 1
                 FROM active_summary_chain AS chain
                 JOIN context_compaction_summary_lineage AS lineage
                   ON lineage.summary_id = chain.id
                  AND lineage.conversation_id = ?1
                 WHERE lineage.introduced_by_assistant_message_id = ?2
             )",
            params![
                &admission.conversation_id,
                &admission.source_assistant_message_id
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if source_owns_active_summary {
        return Err(
            "edit_turn_compacted: the source Turn owns an active context summary lineage"
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn insert_in_transaction(
    connection: &Connection,
    admission: &ConversationTurnRewriteAdmission,
) -> rusqlite::Result<()> {
    let changed = connection.execute(
        "INSERT INTO conversation_turn_rewrites (
             request_id, request_fingerprint, conversation_id,
             source_user_message_id, source_assistant_message_id,
             replacement_user_message_id, replacement_assistant_message_id,
             run_id, response_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            &admission.request_id,
            &admission.request_fingerprint,
            &admission.conversation_id,
            &admission.source_user_message_id,
            &admission.source_assistant_message_id,
            &admission.replacement_user_message_id,
            &admission.replacement_assistant_message_id,
            &admission.run_id,
            &admission.response_json,
            admission.created_at,
        ],
    )?;
    if changed != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

pub(crate) fn superseded_message_ids(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<std::collections::HashSet<String>> {
    let mut statement = connection.prepare(
        "SELECT source_user_message_id, source_assistant_message_id
         FROM conversation_turn_rewrites
         WHERE conversation_id = ?1",
    )?;
    let rows = statement.query_map([conversation_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut ids = std::collections::HashSet::new();
    for row in rows {
        let (user, assistant) = row?;
        ids.insert(user);
        ids.insert(assistant);
    }
    Ok(ids)
}

pub(crate) fn replacement_message_ids_by_source(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<std::collections::HashMap<String, String>> {
    let mut statement = connection.prepare(
        "SELECT source_user_message_id, source_assistant_message_id,
                replacement_user_message_id, replacement_assistant_message_id
         FROM conversation_turn_rewrites
         WHERE conversation_id = ?1
         ORDER BY created_at, request_id",
    )?;
    let rows = statement.query_map([conversation_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut replacements = std::collections::HashMap::new();
    for row in rows {
        let (source_user, source_assistant, replacement_user, replacement_assistant) = row?;
        replacements.insert(source_user, replacement_user);
        replacements.insert(source_assistant, replacement_assistant);
    }
    Ok(replacements)
}

pub(crate) fn resolve_active_message_id(
    replacements: &std::collections::HashMap<String, String>,
    source_id: &str,
) -> Result<String, String> {
    let mut resolved = source_id.to_string();
    let mut visited = std::collections::HashSet::new();
    while let Some(replacement) = replacements.get(&resolved) {
        if !visited.insert(resolved.clone()) {
            return Err(
                "Conversation Turn rewrite replacement lineage contains a cycle".to_string(),
            );
        }
        resolved = replacement.clone();
    }
    Ok(resolved)
}
