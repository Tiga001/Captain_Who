use super::*;
use crate::conversation_trace::{
    validate_model_item_against_trace, ConversationTraceValidationState,
};
use crate::{ConversationTracePublication, ConversationTurnTraceTerminalStatus};
use rusqlite::{params, Connection, TransactionBehavior};

/// Process-only capability for one immutable Runtime publication lineage. Any failure consumes
/// its authority; the next publication must rebuild from durable journals. It is never serialized.
#[derive(Default)]
pub struct ConversationTraceCommitCursor {
    committed: Option<CommittedPublication>,
}

struct CommittedPublication {
    conversation_id: String,
    assistant_message_id: String,
    run_id: String,
    database_instance: Arc<()>,
    epoch: String,
    revision: i64,
    publication: ConversationTracePublication,
    validation: ConversationTraceValidationState,
}

impl StorageService {
    #[allow(clippy::too_many_arguments)]
    pub fn append_trusted_conversation_trace_publication(
        &self,
        cursor: &mut ConversationTraceCommitCursor,
        publication: &ConversationTracePublication,
        conversation_id: &str,
        assistant_message_id: &str,
        run_id: &str,
        created_at: i64,
        updated_at: i64,
    ) -> Result<ConversationTraceAppendOutcome, String> {
        // Taking authority before *any* fallible operation makes validation, I/O and commit-
        // unknown failures equally safe. A retry must re-establish the complete durable prefix.
        let previous = cursor.committed.take();
        #[cfg(test)]
        let lock_started = std::time::Instant::now();
        let mut connection = self.state.connection()?;
        #[cfg(test)]
        crate::storage::trace_performance_metrics::lock_wait(lock_started.elapsed());
        #[cfg(test)]
        let transaction_started = std::time::Instant::now();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let database_instance = self.state.instance_identity.clone();
        let current_revision =
            journal_revision(&transaction, conversation_id, assistant_message_id, run_id)?;
        let hot = previous.filter(|previous| {
            previous.conversation_id == conversation_id
                && previous.assistant_message_id == assistant_message_id
                && previous.run_id == run_id
                && Arc::ptr_eq(&previous.database_instance, &database_instance)
                && current_revision.as_ref() == Some(&(previous.epoch.clone(), previous.revision))
                && ((publication
                    .parent
                    .as_ref()
                    .is_some_and(|parent| Arc::ptr_eq(parent, &previous.publication.identity))
                    && publication.prefix_trace_count == previous.publication.trace_items.len()
                    && publication.prefix_model_count == previous.publication.model_items.len())
                    || Arc::ptr_eq(&publication.identity, &previous.publication.identity))
        });
        // Guidance rows have their own state machine outside the trace journal. Keep the old
        // cross-journal check without reloading their (potentially large) content. A changed or
        // missing row must go through the complete transactional replay/conflict path.
        let hot = match hot {
            Some(previous) if guidance_prefix_is_applied(&transaction, &previous.publication)? => {
                Some(previous)
            }
            _ => None,
        };
        let (outcome, validation) = if let Some(mut previous) = hot {
            if updated_at < created_at {
                return Err("conversation trace commit time cannot precede created_at".into());
            }
            if previous.publication.is_truncated() && !publication.is_truncated() {
                return Err("conversation trace truncated state cannot be cleared".into());
            }
            let trace_count = previous.publication.trace_items.len();
            let model_count = previous.publication.model_items.len();
            previous.validation.append(
                publication.trace_items[trace_count..]
                    .iter()
                    .map(AsRef::as_ref),
                ConversationTurnTraceTerminalStatus::InProgress,
            )?;
            validate_model_suffix(publication, model_count)?;
            let changed = trace_count != publication.trace_items.len()
                || model_count != publication.model_items.len()
                || previous.publication.is_truncated() != publication.is_truncated();
            for item in &publication.trace_items[trace_count..] {
                conversation_trace_repository::insert_trace_item(
                    &transaction,
                    assistant_message_id,
                    item,
                )
                .map_err(storage_error)?;
            }
            conversation_model_context_repository::insert_validated_suffix(
                &transaction,
                assistant_message_id,
                publication.model_items[model_count..]
                    .iter()
                    .map(AsRef::as_ref),
            )
            .map_err(storage_error)?;
            // Async guidance requires both its exact trace and model-context proof. Publish
            // both journals before changing delivery state, within the same transaction.
            for item in &publication.trace_items[trace_count..] {
                apply_guidance(&transaction, item, updated_at)?;
            }
            if changed {
                transaction.execute("UPDATE conversation_turn_traces SET truncated=?1, updated_at=?2 WHERE assistant_message_id=?3",
                    params![publication.is_truncated(), updated_at, assistant_message_id]).map_err(storage_error)?;
            }
            provider_continuation_repository::promote_staged_trace_projections_in_connection(
                &transaction,
                conversation_id,
                assistant_message_id,
                run_id,
                updated_at,
            )
            .map_err(storage_error)?;
            (
                ConversationTraceAppendOutcome {
                    changed,
                    previous_trace: None,
                    previous_model_context_items: Vec::new(),
                    previous_publication: Some(previous.publication),
                },
                previous.validation,
            )
        } else {
            let trace =
                publication.in_progress_audit_trace(run_id, conversation_id, assistant_message_id);
            let outcome = messages::append_trace_with_previous_projection_in_transaction(
                &transaction,
                &trace,
                &publication.model_context_items,
                created_at,
                updated_at,
            )?;
            let mut validation = ConversationTraceValidationState::default();
            validation.append(
                publication.trace_items.iter().map(AsRef::as_ref),
                ConversationTurnTraceTerminalStatus::InProgress,
            )?;
            (outcome, validation)
        };
        let (epoch, revision) =
            journal_revision(&transaction, conversation_id, assistant_message_id, run_id)?
                .ok_or_else(|| {
                    "committed Trace publication has no active journal revision".to_string()
                })?;
        transaction.commit().map_err(storage_error)?;
        #[cfg(test)]
        crate::storage::trace_performance_metrics::transaction(transaction_started.elapsed());
        cursor.committed = Some(CommittedPublication {
            conversation_id: conversation_id.to_owned(),
            assistant_message_id: assistant_message_id.to_owned(),
            run_id: run_id.to_owned(),
            database_instance,
            epoch,
            revision,
            publication: publication.clone(),
            validation,
        });
        Ok(outcome)
    }
}

fn journal_revision(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    run_id: &str,
) -> Result<Option<(String, i64)>, String> {
    connection.query_row(
        "SELECT revision.epoch, revision.revision FROM conversation_trace_journal_revisions AS revision
         JOIN conversation_turn_traces AS trace ON trace.assistant_message_id=revision.assistant_message_id
         JOIN messages AS message ON message.id=trace.assistant_message_id
         WHERE trace.assistant_message_id=?1 AND trace.conversation_id=?2 AND trace.run_id=?3
           AND trace.terminal_status='in_progress' AND message.conversation_id=?2 AND message.role='assistant'",
        params![message_id, conversation_id, run_id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(storage_error)
}

fn guidance_prefix_is_applied(
    connection: &Connection,
    publication: &ConversationTracePublication,
) -> Result<bool, String> {
    for item in &publication.trace_items {
        let ConversationTurnTraceItem::UserGuidance {
            guidance_id,
            sequence,
            ..
        } = item.as_ref()
        else {
            continue;
        };
        let applied: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_run_guidances WHERE guidance_id=?1 AND status='applied' AND applied_trace_sequence=?2)",
            params![guidance_id, sequence], |row| row.get(0),
        ).map_err(storage_error)?;
        if !applied {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_model_suffix(
    publication: &ConversationTracePublication,
    previous_count: usize,
) -> Result<(), String> {
    let mut previous = previous_count.checked_sub(1).map(|index| {
        let item = &publication.model_items[index];
        (item.sequence, item.ordinal)
    });
    let previous_sequence = previous.map(|identity| identity.0);
    let mut covered = std::collections::BTreeSet::new();
    for item in &publication.model_items[previous_count..] {
        item.validate()?;
        let identity = (item.sequence, item.ordinal);
        if previous.is_some_and(|previous| identity <= previous) {
            return Err("model context item identity must be strictly increasing".into());
        }
        previous = Some(identity);
        let index = publication
            .trace_items
            .binary_search_by_key(&item.sequence, |item| item.sequence())
            .map_err(|_| "model context item references a missing trace sequence".to_string())?;
        validate_model_item_against_trace(item, &publication.trace_items[index])?;
        if previous_sequence.is_none_or(|sequence| item.sequence > sequence) {
            covered.insert(item.sequence);
        }
    }
    let Some(covered_through) = previous.map(|identity| identity.0) else {
        return Ok(());
    };
    let start = publication.trace_items.partition_point(|item| {
        previous_sequence.is_some_and(|sequence| item.sequence() <= sequence)
    });
    let expected = publication.trace_items[start..]
        .iter()
        .take_while(|item| item.sequence() <= covered_through)
        .filter(|item| item.is_model_visible())
        .map(|item| item.sequence())
        .collect::<std::collections::BTreeSet<_>>();
    if covered != expected {
        return Err("model context items must cover a complete contiguous trace prefix".into());
    }
    let boundary = publication
        .trace_items
        .binary_search_by_key(&covered_through, |item| item.sequence())
        .map_err(|_| "model context prefix boundary is missing".to_string())?;
    let boundary_item = &publication.trace_items[boundary];
    let staged_open_call = matches!(**boundary_item, ConversationTurnTraceItem::ToolCall { .. })
        && boundary + 1 == publication.trace_items.len();
    if !boundary_item.is_safe_compaction_boundary() && !staged_open_call {
        return Err("model context prefix cannot end with an unresolved tool call".into());
    }
    Ok(())
}

fn apply_guidance(
    connection: &Connection,
    item: &ConversationTurnTraceItem,
    updated_at: i64,
) -> Result<(), String> {
    let ConversationTurnTraceItem::UserGuidance {
        guidance_id,
        sequence,
        ..
    } = item
    else {
        return Ok(());
    };
    match guidance_repository::mark_guidance_applied(connection, guidance_id, *sequence, updated_at)
        .map_err(storage_error)?
    {
        guidance_repository::AgentRunGuidanceTransitionOutcome::Updated
        | guidance_repository::AgentRunGuidanceTransitionOutcome::Idempotent => Ok(()),
        _ => Err(format!(
            "conversation trace guidance `{guidance_id}` conflicts with its journal"
        )),
    }
}

#[cfg(test)]
#[path = "trace_publication_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "trace_publication_external_tests.rs"]
mod external_tests;
