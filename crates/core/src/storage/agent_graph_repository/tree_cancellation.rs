use super::common::{
    conflict, corrupt, immediate, read_error, validate_id, validate_time, validate_trimmed,
    write_error,
};
use super::node_records::{query_node, query_node_by_root_conversation};
use super::wake_records::{query_wakes, WAKE_SELECT};
use crate::{AgentGraphError, AgentWakeRequestRecord, AgentWakeStatus};
use rusqlite::{params, Connection, OptionalExtension};

const MAX_RUN_ID_BYTES: usize = 2_048;

/// One immutable, run-scoped Agent-tree stop fact.
///
/// The root Run and every admitted descendant Run each own one row. All rows point back to the
/// same `root_run_id`; a later Turn uses a different Run ID and is therefore unaffected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTreeRunStopRecord {
    pub run_id: String,
    pub root_run_id: String,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub stopped_at: i64,
}

/// One admitted descendant Turn that must be interrupted by the Host.
///
/// Pre-admission work never appears here: queued and claimed Wakes are cancelled atomically by
/// [`cancel_agent_tree_wakes_by_root_agent`] before this snapshot is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAgentTreeWake {
    pub agent_id: String,
    pub conversation_id: String,
    pub wake_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub claim_token: String,
    pub status: AgentWakeStatus,
}

/// Durable snapshot for one Agent-tree cancellation boundary.
///
/// The first sweep freezes its exact pending and admitted work. A retry returns only still-active
/// Runs from that frozen membership; it never expands into later user-authorized work. Agent node
/// lifecycle is deliberately untouched, so a later explicit follow-up may wake the same Agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTreeWakeCancellationBatch {
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub cancelled_before_admission: Vec<AgentWakeRequestRecord>,
    pub active_wakes: Vec<ActiveAgentTreeWake>,
}

/// The durable stop fact and the Wake snapshot committed in the same SQLite transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTreeRunStopCancellation {
    pub stop: AgentTreeRunStopRecord,
    pub cancellation: AgentTreeWakeCancellationBatch,
}

/// Cancels all not-yet-admitted descendant work and snapshots every admitted descendant Turn.
///
/// The immediate transaction closes the `claimed -> running` admission race: a Wake is either
/// cancelled while still queued/claimed, or it has already bound a Run and is returned for exact
/// Host cancellation. The root Agent's human-driven Turn is not a Wake and is intentionally not
/// part of this batch.
pub fn cancel_agent_tree_wakes_by_root_agent(
    connection: &mut Connection,
    root_agent_id: &str,
    cancelled_at: i64,
) -> Result<AgentTreeWakeCancellationBatch, AgentGraphError> {
    validate_id("root_agent_id", root_agent_id)?;
    validate_time(cancelled_at)?;
    let transaction = immediate(connection)?;
    let root = query_node(&transaction, root_agent_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(root_agent_id.to_string()))?;
    ensure_root_identity(&root)?;
    let batch = cancel_agent_tree_wakes_in_transaction(&transaction, &root, None, cancelled_at)?;
    transaction.commit().map_err(write_error)?;
    Ok(batch)
}

/// Resolves a root Conversation to its durable root Agent and performs one cancellation sweep.
pub fn cancel_agent_tree_wakes_by_root_conversation(
    connection: &mut Connection,
    root_conversation_id: &str,
    cancelled_at: i64,
) -> Result<AgentTreeWakeCancellationBatch, AgentGraphError> {
    validate_id("root_conversation_id", root_conversation_id)?;
    validate_time(cancelled_at)?;
    let transaction = immediate(connection)?;
    let root = query_node_by_root_conversation(&transaction, root_conversation_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(root_conversation_id.to_string()))?;
    ensure_root_identity(&root)?;
    let batch = cancel_agent_tree_wakes_in_transaction(&transaction, &root, None, cancelled_at)?;
    transaction.commit().map_err(write_error)?;
    Ok(batch)
}

/// Starts or idempotently reinforces a durable stop for one human-driven root Run.
///
/// The stop row, all newly discovered admitted descendant Run rows, and cancellation of all
/// pre-admission descendant Wakes commit atomically under one immediate transaction.
pub fn begin_agent_tree_run_stop_by_root_agent(
    connection: &mut Connection,
    root_agent_id: &str,
    root_run_id: &str,
    stopped_at: i64,
) -> Result<Option<AgentTreeRunStopCancellation>, AgentGraphError> {
    validate_id("root_agent_id", root_agent_id)?;
    validate_run_id(root_run_id)?;
    validate_time(stopped_at)?;
    let transaction = immediate(connection)?;
    let root = query_node(&transaction, root_agent_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(root_agent_id.to_string()))?;
    ensure_root_identity(&root)?;
    let Some((stop, created)) =
        begin_agent_tree_run_stop_in_transaction(&transaction, &root, root_run_id, stopped_at)?
    else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    let cancellation = if created {
        cancel_agent_tree_wakes_in_transaction(&transaction, &root, Some(&stop), stopped_at)?
    } else {
        covered_agent_tree_wakes_in_transaction(&transaction, &root, &stop)?
    };
    transaction.commit().map_err(write_error)?;
    Ok(Some(AgentTreeRunStopCancellation { stop, cancellation }))
}

/// Resolves a root Conversation and starts or idempotently reinforces its exact root Run stop.
pub fn begin_agent_tree_run_stop_by_root_conversation(
    connection: &mut Connection,
    root_conversation_id: &str,
    root_run_id: &str,
    stopped_at: i64,
) -> Result<Option<AgentTreeRunStopCancellation>, AgentGraphError> {
    validate_id("root_conversation_id", root_conversation_id)?;
    validate_run_id(root_run_id)?;
    validate_time(stopped_at)?;
    let transaction = immediate(connection)?;
    let root = query_node_by_root_conversation(&transaction, root_conversation_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(root_conversation_id.to_string()))?;
    ensure_root_identity(&root)?;
    let Some((stop, created)) =
        begin_agent_tree_run_stop_in_transaction(&transaction, &root, root_run_id, stopped_at)?
    else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    let cancellation = if created {
        cancel_agent_tree_wakes_in_transaction(&transaction, &root, Some(&stop), stopped_at)?
    } else {
        covered_agent_tree_wakes_in_transaction(&transaction, &root, &stop)?
    };
    transaction.commit().map_err(write_error)?;
    Ok(Some(AgentTreeRunStopCancellation { stop, cancellation }))
}

/// Repeats a stopped tree's durable sweep when `run_id` is either its root Run or any covered
/// descendant Run. Unknown Runs are a normal `None`, allowing callers to distinguish an ordinary
/// single-Agent interrupt from a user root-tree stop.
pub fn reinforce_agent_tree_run_stop(
    connection: &mut Connection,
    run_id: &str,
    cancelled_at: i64,
) -> Result<Option<AgentTreeRunStopCancellation>, AgentGraphError> {
    validate_run_id(run_id)?;
    validate_time(cancelled_at)?;
    let transaction = immediate(connection)?;
    let Some(covered) = query_agent_tree_run_stop_in_connection(&transaction, run_id)? else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    let stop = query_agent_tree_run_stop_in_connection(&transaction, &covered.root_run_id)?
        .ok_or_else(|| corrupt("Agent-tree stop root fact is missing"))?;
    if stop.run_id != stop.root_run_id
        || stop.root_agent_id != covered.root_agent_id
        || stop.root_conversation_id != covered.root_conversation_id
        || stop.stopped_at != covered.stopped_at
    {
        return Err(corrupt("Agent-tree stop membership escaped its root fact"));
    }
    let root = query_node(&transaction, &stop.root_agent_id)?
        .ok_or_else(|| corrupt("stopped Agent-tree root is missing"))?;
    ensure_root_identity(&root)?;
    let cancellation = covered_agent_tree_wakes_in_transaction(&transaction, &root, &stop)?;
    transaction.commit().map_err(write_error)?;
    Ok(Some(AgentTreeRunStopCancellation { stop, cancellation }))
}

/// Looks up the immutable tree-stop fact covering this exact Run ID.
pub fn get_agent_tree_run_stop(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<AgentTreeRunStopRecord>, AgentGraphError> {
    validate_run_id(run_id)?;
    query_agent_tree_run_stop_in_connection(connection, run_id)
}

/// Transaction-level scheduling guard used by Spawn and Followup.
///
/// Callers must invoke this after acquiring their own immediate transaction. This makes both
/// commit orders safe: scheduling first is caught by the subsequent stop sweep, while stop first
/// is rejected here before any message, node, Conversation, or Wake is inserted.
pub(crate) fn ensure_agent_tree_origin_run_can_schedule_in_transaction(
    connection: &Connection,
    origin_run_id: &str,
    scheduling_agent_id: &str,
) -> Result<(), AgentGraphError> {
    validate_run_id(origin_run_id)?;
    validate_id("scheduling_agent_id", scheduling_agent_id)?;
    let owns_run = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM conversation_turn_traces AS trace
                 JOIN agent_nodes AS agent ON agent.conversation_id = trace.conversation_id
                 WHERE trace.run_id = ?1 AND agent.agent_id = ?2
             )",
            params![origin_run_id, scheduling_agent_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if !owns_run {
        return Err(conflict("origin Run is not bound to the scheduling Agent"));
    }
    if query_agent_tree_run_stop_in_connection(connection, origin_run_id)?.is_some() {
        return Err(conflict("origin Agent Run has been stopped"));
    }
    Ok(())
}

fn ensure_root_identity(root: &crate::AgentNodeRecord) -> Result<(), AgentGraphError> {
    if root.parent_agent_id.is_some()
        || root.agent_id != root.root_agent_id
        || root.conversation_id != root.root_conversation_id
    {
        return Err(conflict("Agent-tree cancellation requires a root Agent"));
    }
    Ok(())
}

fn validate_run_id(run_id: &str) -> Result<(), AgentGraphError> {
    validate_trimmed("run_id", run_id, MAX_RUN_ID_BYTES)
}

fn begin_agent_tree_run_stop_in_transaction(
    transaction: &Connection,
    root: &crate::AgentNodeRecord,
    root_run_id: &str,
    stopped_at: i64,
) -> Result<Option<(AgentTreeRunStopRecord, bool)>, AgentGraphError> {
    if let Some(existing) = query_agent_tree_run_stop_in_connection(transaction, root_run_id)? {
        if existing.run_id == root_run_id
            && existing.root_run_id == root_run_id
            && existing.root_agent_id == root.agent_id
            && existing.root_conversation_id == root.root_conversation_id
        {
            return Ok(Some((existing, false)));
        }
        return Err(conflict(
            "root Run is already covered by another Agent-tree stop",
        ));
    }
    let root_trace_status = transaction
        .query_row(
            "SELECT terminal_status
             FROM conversation_turn_traces
             WHERE run_id = ?1 AND conversation_id = ?2",
            params![root_run_id, &root.root_conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    if root_trace_status.as_deref() != Some("in_progress") {
        return Ok(None);
    }
    transaction
        .execute(
            "INSERT INTO agent_tree_run_stops (
                 run_id, root_run_id, root_agent_id, root_conversation_id, stopped_at
             ) VALUES (?1, ?1, ?2, ?3, ?4)",
            params![
                root_run_id,
                &root.agent_id,
                &root.root_conversation_id,
                stopped_at,
            ],
        )
        .map_err(write_error)?;
    record_stop_membership_in_transaction(transaction, root_run_id, root_run_id)?;
    query_agent_tree_run_stop_in_connection(transaction, root_run_id)?
        .map(|stop| Some((stop, true)))
        .ok_or_else(|| corrupt("created Agent-tree stop root fact disappeared"))
}

pub(super) fn query_agent_tree_run_stop_in_connection(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<AgentTreeRunStopRecord>, AgentGraphError> {
    connection
        .query_row(
            "SELECT run_id, root_run_id, root_agent_id, root_conversation_id, stopped_at
             FROM agent_tree_run_stops
             WHERE run_id = ?1",
            [run_id],
            |row| {
                Ok(AgentTreeRunStopRecord {
                    run_id: row.get(0)?,
                    root_run_id: row.get(1)?,
                    root_agent_id: row.get(2)?,
                    root_conversation_id: row.get(3)?,
                    stopped_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(read_error)
}

fn record_covered_run_in_transaction(
    transaction: &Connection,
    stop: &AgentTreeRunStopRecord,
    run_id: &str,
) -> Result<(), AgentGraphError> {
    if let Some(existing) = query_agent_tree_run_stop_in_connection(transaction, run_id)? {
        if existing.root_agent_id == stop.root_agent_id
            && existing.root_conversation_id == stop.root_conversation_id
        {
            // A descendant may still be converging from an earlier stopped root Turn when the
            // user starts and then stops a later root Turn. Its immutable first stop fact already
            // provides the required settlement/recovery fence. Record that the later exact stop
            // also observed this Run so its retries neither lose it nor absorb a still-later Turn.
            return record_stop_membership_in_transaction(transaction, &stop.root_run_id, run_id);
        }
        return Err(conflict(
            "Agent Run is already covered by another tree-stop fact",
        ));
    }
    transaction
        .execute(
            "INSERT INTO agent_tree_run_stops (
                 run_id, root_run_id, root_agent_id, root_conversation_id, stopped_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run_id,
                &stop.root_run_id,
                &stop.root_agent_id,
                &stop.root_conversation_id,
                stop.stopped_at,
            ],
        )
        .map_err(write_error)?;
    record_stop_membership_in_transaction(transaction, &stop.root_run_id, run_id)
}

fn record_stop_membership_in_transaction(
    transaction: &Connection,
    root_run_id: &str,
    run_id: &str,
) -> Result<(), AgentGraphError> {
    transaction
        .execute(
            "INSERT INTO agent_tree_run_stop_members (root_run_id, run_id)
             VALUES (?1, ?2)
             ON CONFLICT(root_run_id, run_id) DO NOTHING",
            params![root_run_id, run_id],
        )
        .map_err(write_error)?;
    Ok(())
}

fn cancel_agent_tree_wakes_in_transaction(
    transaction: &Connection,
    root: &crate::AgentNodeRecord,
    durable_stop: Option<&AgentTreeRunStopRecord>,
    cancelled_at: i64,
) -> Result<AgentTreeWakeCancellationBatch, AgentGraphError> {
    let pending = query_wakes(
        transaction,
        &format!(
            "{WAKE_SELECT}
             WHERE root_agent_id = ?1
               AND agent_id != ?1
               AND status IN ('queued', 'claimed')
               AND run_id IS NULL
             ORDER BY sequence, wake_id"
        ),
        [&root.agent_id],
    )?;

    let changed = transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'cancelled', status_revision = status_revision + 1,
                 completed_at = ?1
             WHERE root_agent_id = ?2
               AND agent_id != ?2
               AND status IN ('queued', 'claimed')
               AND run_id IS NULL",
            params![cancelled_at, &root.agent_id],
        )
        .map_err(write_error)?;
    if changed != pending.len() {
        return Err(conflict(
            "Agent-tree pending Wake cancellation lost its durable boundary",
        ));
    }

    let mut cancelled_before_admission = pending;
    for wake in &mut cancelled_before_admission {
        wake.status = AgentWakeStatus::Cancelled;
        wake.status_revision = wake
            .status_revision
            .checked_add(1)
            .ok_or_else(|| corrupt("Wake status revision overflowed"))?;
        wake.completed_at = Some(cancelled_at);
    }

    let admitted = query_wakes(
        transaction,
        &format!(
            "{WAKE_SELECT}
             WHERE root_agent_id = ?1
               AND agent_id != ?1
               AND (
                   status IN ('running', 'waiting_for_approval')
                   OR (status = 'claimed' AND run_id IS NOT NULL)
               )
             ORDER BY sequence, wake_id"
        ),
        [&root.agent_id],
    )?;
    let mut active_wakes = Vec::with_capacity(admitted.len());
    for wake in admitted {
        let run_id = wake
            .run_id
            .ok_or_else(|| corrupt("admitted descendant Wake is missing its Run identity"))?;
        let assistant_message_id = wake
            .assistant_message_id
            .ok_or_else(|| corrupt("admitted descendant Wake is missing its Assistant identity"))?;
        let claim_token = wake
            .claim_token
            .ok_or_else(|| corrupt("admitted descendant Wake is missing its claim token"))?;
        let agent = query_node(transaction, &wake.agent_id)?
            .ok_or_else(|| corrupt("descendant Wake Agent is missing"))?;
        if agent.root_agent_id != root.agent_id {
            return Err(corrupt("descendant Wake escaped its Agent tree"));
        }
        if let Some(stop) = durable_stop {
            record_covered_run_in_transaction(transaction, stop, &run_id)?;
        }
        active_wakes.push(ActiveAgentTreeWake {
            agent_id: wake.agent_id,
            conversation_id: agent.conversation_id,
            wake_id: wake.wake_id,
            run_id,
            assistant_message_id,
            claim_token,
            status: wake.status,
        });
    }

    Ok(AgentTreeWakeCancellationBatch {
        root_agent_id: root.agent_id.clone(),
        root_conversation_id: root.root_conversation_id.clone(),
        cancelled_before_admission,
        active_wakes,
    })
}

/// Returns only still-active Runs frozen by a stop transaction in this exact Agent tree.
///
/// A retry must never broaden its scope to later user-authorized work in the same reusable Agent
/// tree. The immutable membership table records exactly which admitted Runs this root stop
/// observed, including a still-converging Run first fenced by an older stop.
fn covered_agent_tree_wakes_in_transaction(
    transaction: &Connection,
    root: &crate::AgentNodeRecord,
    stop: &AgentTreeRunStopRecord,
) -> Result<AgentTreeWakeCancellationBatch, AgentGraphError> {
    let admitted = query_wakes(
        transaction,
        &format!(
            "{WAKE_SELECT}
             WHERE root_agent_id = ?1
               AND agent_id != ?1
               AND run_id IN (
                   SELECT run_id FROM agent_tree_run_stop_members
                   WHERE root_run_id = ?2
               )
               AND (
                   status IN ('running', 'waiting_for_approval')
                   OR (status = 'claimed' AND run_id IS NOT NULL)
               )
             ORDER BY sequence, wake_id"
        ),
        [&root.agent_id, &stop.root_run_id],
    )?;
    let mut active_wakes = Vec::with_capacity(admitted.len());
    for wake in admitted {
        let run_id = wake
            .run_id
            .ok_or_else(|| corrupt("covered descendant Wake is missing its Run identity"))?;
        let assistant_message_id = wake
            .assistant_message_id
            .ok_or_else(|| corrupt("covered descendant Wake is missing its Assistant identity"))?;
        let claim_token = wake
            .claim_token
            .ok_or_else(|| corrupt("covered descendant Wake is missing its claim token"))?;
        let agent = query_node(transaction, &wake.agent_id)?
            .ok_or_else(|| corrupt("covered descendant Wake Agent is missing"))?;
        if agent.root_agent_id != root.agent_id
            || agent.root_conversation_id != root.root_conversation_id
        {
            return Err(corrupt("covered descendant Wake escaped its Agent tree"));
        }
        active_wakes.push(ActiveAgentTreeWake {
            agent_id: wake.agent_id,
            conversation_id: agent.conversation_id,
            wake_id: wake.wake_id,
            run_id,
            assistant_message_id,
            claim_token,
            status: wake.status,
        });
    }
    Ok(AgentTreeWakeCancellationBatch {
        root_agent_id: root.agent_id.clone(),
        root_conversation_id: root.root_conversation_id.clone(),
        cancelled_before_admission: Vec::new(),
        active_wakes,
    })
}
