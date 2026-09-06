use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct ConversationWorldStateCommitRequest<'a> {
    pub conversation_id: &'a str,
    pub boundary: &'a WorldStateRequestBoundary,
    pub expected_head: Option<&'a WorldStateSnapshot>,
    pub owned_section_ids: &'a [WorldStateSectionId],
    pub sections: &'a [WorldStateSectionEnvelope],
    pub initial_epoch_id: &'a str,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationWorldStateCommitOutcome {
    pub append_outcome: ConversationWorldStateAppendOutcome,
    pub records: Vec<AnchoredWorldStateRecord>,
    pub head_snapshot: WorldStateSnapshot,
    pub boundary: WorldStateRequestBoundary,
}

/// Atomically replaces only the declared owner's sections and records a prepared request identity.
/// The receipt exists even when the state is unchanged, and makes retries independent of the clock.
pub fn commit_request(
    connection: &mut Connection,
    request: &ConversationWorldStateCommitRequest<'_>,
) -> Result<ConversationWorldStateCommitOutcome, ConversationWorldStateRepositoryError> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let result = commit_request_in_connection(&transaction, request)?;
    transaction.commit()?;
    Ok(result)
}

fn commit_request_in_connection(
    connection: &Connection,
    request: &ConversationWorldStateCommitRequest<'_>,
) -> Result<ConversationWorldStateCommitOutcome, ConversationWorldStateRepositoryError> {
    request.boundary.validate().map_err(invalid_domain)?;
    validate_live_request(connection, request.conversation_id, request.boundary)?;
    let owned = request
        .owned_section_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if owned.len() != request.owned_section_ids.len() || request.created_at < 0 {
        return Err(ConversationWorldStateRepositoryError::Invalid(
            "重复 owner section 或无效时间。".into(),
        ));
    }
    let mut sections = BTreeMap::new();
    for section in request.sections {
        section.validate().map_err(invalid_domain)?;
        if section.lifetime != crate::WorldStateLifetime::Conversation
            || !owned.contains(&section.id)
            || sections
                .insert(section.id.clone(), section.clone())
                .is_some()
        {
            return Err(ConversationWorldStateRepositoryError::Invalid(
                "提交必须具有唯一、owner 范围内的 conversation sections。".into(),
            ));
        }
    }
    let payload = canonicalize_json(
        &serde_json::json!({
            "boundary": request.boundary,
            "ownedSectionIds": owned,
            "sections": sections.values().collect::<Vec<_>>(),
        })
        .to_string(),
    )?;
    let existing = connection
        .query_row(
            "SELECT payload_json FROM conversation_world_state_request_commits
         WHERE conversation_id=?1 AND run_id=?2 AND request_index=?3",
            params![
                request.conversation_id,
                request.boundary.run_id,
                sqlite_u64(request.boundary.request_index, "request index")?
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing != payload {
            return Err(ConversationWorldStateRepositoryError::Conflict(
                "同一请求边界已准备了不同的 World State。".into(),
            ));
        }
        return commit_outcome(
            connection,
            request,
            ConversationWorldStateAppendOutcome::Idempotent,
        );
    }
    validate_request_order(connection, request.conversation_id, request.boundary)?;
    let current = fold_active_snapshot(connection, request.conversation_id)?;
    if current.as_ref() != request.expected_head {
        return Err(ConversationWorldStateRepositoryError::Conflict(
            "World State head CAS 已变化。".into(),
        ));
    }
    let epoch = get_active_epoch(connection, request.conversation_id)?;
    let initial_summary = if epoch.is_none() {
        crate::storage::context_compaction_repository::get_active_summary(
            connection,
            request.conversation_id,
        )
        .map_err(|error| ConversationWorldStateRepositoryError::Invalid(error.to_string()))?
    } else {
        None
    };
    let record = match current.as_ref() {
        None => WorldStateRecord::Full(
            WorldStateSnapshot::new(
                request.initial_epoch_id,
                0,
                sections.into_values().collect(),
            )
            .map_err(invalid_domain)?,
        ),
        Some(current) => {
            let mut merged = current
                .sections
                .iter()
                .filter(|s| !owned.contains(&s.id))
                .cloned()
                .map(|s| (s.id.clone(), s))
                .collect::<BTreeMap<_, _>>();
            merged.extend(sections);
            let next = WorldStateSnapshot::new(
                &current.epoch_id,
                current.sequence.checked_add(1).ok_or_else(|| {
                    ConversationWorldStateRepositoryError::Invalid(
                        "World State sequence 溢出。".into(),
                    )
                })?,
                merged.into_values().collect(),
            )
            .map_err(invalid_domain)?;
            if next.revision == current.revision {
                insert_prepared_receipt(connection, request, &payload, current)?;
                return commit_outcome(
                    connection,
                    request,
                    ConversationWorldStateAppendOutcome::Inserted,
                );
            }
            WorldStateRecord::Diff(WorldStateDiff::between(current, &next).map_err(invalid_domain)?)
        }
    };
    append_record_in_connection(
        connection,
        &ConversationWorldStateRecordWrite {
            conversation_id: request.conversation_id,
            epoch_generation: epoch.as_ref().map_or(1, |e| e.generation),
            base_summary_id: epoch
                .as_ref()
                .and_then(|e| e.base_summary_id.as_deref())
                .or_else(|| initial_summary.as_ref().map(|summary| summary.id.as_str())),
            effective_before_message_id: None,
            request_boundary: matches!(&record, WorldStateRecord::Diff(_))
                .then_some(request.boundary),
            model_observed: false,
            record: &record,
            created_at: request.created_at,
        },
    )?;
    let head = fold_active_snapshot(connection, request.conversation_id)?.ok_or_else(|| {
        ConversationWorldStateRepositoryError::Corrupt("提交后 World State head 缺失。".into())
    })?;
    insert_prepared_receipt(connection, request, &payload, &head)?;
    commit_outcome(
        connection,
        request,
        ConversationWorldStateAppendOutcome::Inserted,
    )
}

fn insert_prepared_receipt(
    connection: &Connection,
    request: &ConversationWorldStateCommitRequest<'_>,
    payload: &str,
    head: &WorldStateSnapshot,
) -> Result<(), ConversationWorldStateRepositoryError> {
    connection.execute(
        "INSERT INTO conversation_world_state_request_commits
         (conversation_id, run_id, assistant_message_id, request_index, after_trace_sequence, payload_json, epoch_id, sequence, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![request.conversation_id, request.boundary.run_id, request.boundary.assistant_message_id,
            sqlite_u64(request.boundary.request_index, "request index")?, request.boundary.after_trace_sequence.map(|s| sqlite_u64(s, "trace sequence")).transpose()?,
            payload, head.epoch_id, sqlite_u64(head.sequence, "world state sequence")?, request.created_at],
    )?;
    Ok(())
}

fn commit_outcome(
    connection: &Connection,
    request: &ConversationWorldStateCommitRequest<'_>,
    append_outcome: ConversationWorldStateAppendOutcome,
) -> Result<ConversationWorldStateCommitOutcome, ConversationWorldStateRepositoryError> {
    Ok(ConversationWorldStateCommitOutcome {
        append_outcome,
        records: list_active_journal_entries(connection, request.conversation_id)?
            .iter()
            .map(|entry| entry.anchored_record())
            .collect(),
        head_snapshot: fold_active_snapshot(connection, request.conversation_id)?.ok_or_else(
            || {
                ConversationWorldStateRepositoryError::Corrupt(
                    "prepared receipt 没有 World State head。".into(),
                )
            },
        )?,
        boundary: request.boundary.clone(),
    })
}

fn validate_live_request(
    connection: &Connection,
    conversation_id: &str,
    boundary: &WorldStateRequestBoundary,
) -> Result<(), ConversationWorldStateRepositoryError> {
    let trace = crate::storage::conversation_trace_repository::get_trace_for_message(
        connection,
        &boundary.assistant_message_id,
    )?
    .ok_or_else(|| {
        ConversationWorldStateRepositoryError::Invalid("请求没有已提交的运行 trace。".into())
    })?;
    if trace.conversation_id != conversation_id
        || trace.run_id != boundary.run_id
        || trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress
    {
        return Err(ConversationWorldStateRepositoryError::Conflict(
            "请求的会话、assistant、run 或运行状态不匹配。".into(),
        ));
    }
    let stopped = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_tree_run_stops WHERE run_id=?1)",
        [&boundary.run_id],
        |row| row.get::<_, bool>(0),
    )?;
    if stopped {
        return Err(ConversationWorldStateRepositoryError::Conflict(
            "运行已停止，拒绝准备 World State 请求。".into(),
        ));
    }
    let latest_safe = trace
        .items
        .iter()
        .filter(|item| item.is_model_visible() && item.is_safe_compaction_boundary())
        .map(|item| item.sequence())
        .max();
    if latest_safe != boundary.after_trace_sequence
        || trace.items.iter().any(|item| {
            item.is_model_visible()
                && boundary
                    .after_trace_sequence
                    .is_none_or(|s| item.sequence() > s)
        })
    {
        return Err(ConversationWorldStateRepositoryError::Conflict(
            "请求边界必须位于最新已提交安全 Trace 前缀之后。".into(),
        ));
    }
    Ok(())
}

fn validate_request_order(
    connection: &Connection,
    conversation_id: &str,
    boundary: &WorldStateRequestBoundary,
) -> Result<(), ConversationWorldStateRepositoryError> {
    let latest = connection.query_row("SELECT request_index, after_trace_sequence FROM conversation_world_state_request_commits
         WHERE conversation_id=?1 AND run_id=?2 ORDER BY request_index DESC LIMIT 1", params![conversation_id, boundary.run_id],
         |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Option<u64>>(1)?))).optional()?;
    if latest.is_some_and(|(index, after)| {
        boundary.request_index <= index || boundary.after_trace_sequence < after
    }) {
        return Err(ConversationWorldStateRepositoryError::Conflict(
            "请求编号或已提交 Trace 前缀不能倒退。".into(),
        ));
    }
    Ok(())
}

/// Acknowledges only the immutable prefix included in this prepared request. A later commit is
/// never acknowledged by an earlier response. Observation metadata is independent of record JSON.
pub fn mark_request_observed(
    connection: &mut Connection,
    conversation_id: &str,
    boundary: &WorldStateRequestBoundary,
) -> Result<(), ConversationWorldStateRepositoryError> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let receipt = transaction.query_row(
        "SELECT epoch_id, sequence, assistant_message_id, after_trace_sequence
         FROM conversation_world_state_request_commits WHERE conversation_id=?1 AND run_id=?2 AND request_index=?3",
        params![conversation_id, boundary.run_id, sqlite_u64(boundary.request_index, "request index")?],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<u64>>(3)?)),
    ).optional()?.ok_or_else(|| ConversationWorldStateRepositoryError::Invalid("无法确认未准备的 World State 请求。".into()))?;
    if receipt.2 != boundary.assistant_message_id || receipt.3 != boundary.after_trace_sequence {
        return Err(ConversationWorldStateRepositoryError::Conflict(
            "World State 确认边界身份不匹配。".into(),
        ));
    }
    transaction.execute("UPDATE conversation_world_state_records SET model_observed=1 WHERE conversation_id=?1 AND epoch_id=?2 AND sequence<=?3", params![conversation_id, receipt.0, sqlite_u64(receipt.1, "world state sequence")?])?;
    transaction.execute(
        "UPDATE conversation_world_state_records AS target SET model_observed=1
        WHERE target.conversation_id=?1 AND target.request_run_id IS NOT NULL AND EXISTS (
            SELECT 1 FROM conversation_world_state_records AS source
            WHERE source.conversation_id=target.conversation_id AND source.model_observed=1
              AND source.request_run_id=target.request_run_id
              AND source.request_assistant_message_id=target.request_assistant_message_id
              AND source.request_index=target.request_index
        )",
        [conversation_id],
    )?;
    transaction.commit()?;
    Ok(())
}

fn invalid_domain(error: crate::WorldStateError) -> ConversationWorldStateRepositoryError {
    ConversationWorldStateRepositoryError::Invalid(error.to_string())
}

/// Inclusive cursor coverage: a request after trace N is outside a cutoff at N itself.
pub(crate) fn record_is_covered(
    connection: &Connection,
    conversation_id: &str,
    before_message: Option<&str>,
    boundary: Option<&WorldStateRequestBoundary>,
    cursor: &crate::ContextJournalCursor,
) -> Result<bool, ConversationWorldStateRepositoryError> {
    let Some(message_id) =
        before_message.or_else(|| boundary.map(|b| b.assistant_message_id.as_str()))
    else {
        return Ok(true);
    };
    let anchor_position = message_position(
        connection,
        conversation_id,
        message_id,
        "World State anchor",
    )?;
    let cutoff_position = message_position(
        connection,
        conversation_id,
        cursor.message_id(),
        "World State cutoff",
    )?;
    if anchor_position != cutoff_position {
        return Ok(anchor_position < cutoff_position);
    }
    Ok(match (boundary, cursor.trace_sequence()) {
        (Some(boundary), Some(cutoff)) => boundary
            .after_trace_sequence
            .is_none_or(|after| after < cutoff),
        _ => true,
    })
}

pub(super) fn validate_anchor_order(
    connection: &Connection,
    record: &IndexedWorldStateRecordWrite<'_>,
) -> Result<(), ConversationWorldStateRepositoryError> {
    let Some(head) = get_active_head(connection, record.conversation_id)? else {
        return Ok(());
    };
    let Some(new_message) = record.effective_before_message_id.or_else(|| {
        record
            .request_boundary
            .map(|b| b.assistant_message_id.as_str())
    }) else {
        return Ok(());
    };
    let Some(old_message) = head.effective_before_message_id.as_deref().or_else(|| {
        head.request_boundary
            .as_ref()
            .map(|b| b.assistant_message_id.as_str())
    }) else {
        return Ok(());
    };
    let new_position = message_position(
        connection,
        record.conversation_id,
        new_message,
        "World State anchor",
    )?;
    let old_position = message_position(
        connection,
        record.conversation_id,
        old_message,
        "World State head anchor",
    )?;
    let backwards = new_position < old_position
        || new_position == old_position
            && match (&head.request_boundary, record.request_boundary) {
                (Some(_), None) => true,
                (Some(old), Some(new)) => {
                    old.run_id != new.run_id
                        || new.after_trace_sequence < old.after_trace_sequence
                        || new.request_index <= old.request_index
                }
                _ => false,
            };
    if backwards {
        return Err(ConversationWorldStateRepositoryError::Conflict(
            "World State anchor 不得倒退或重复。".into(),
        ));
    }
    Ok(())
}
