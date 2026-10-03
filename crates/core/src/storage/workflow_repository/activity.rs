//! Organization activity is a read projection of admitted Turn intervals, not another clock.
//! Frozen admission identities avoid assigning pre-membership history to a new organization.
use super::{storage_error, Error};
use crate::workflow_management::Activity;
use rusqlite::Connection;
use std::collections::HashMap;

pub(super) fn latest_for_instances(
    connection: &Connection,
    instance_ids: &[&str],
) -> Result<HashMap<String, Activity>, Error> {
    if instance_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let requested = serde_json::to_string(instance_ids).map_err(storage_error)?;
    // Child admissions do not receive organization tools or snapshots. A persisted wake's
    // exact parent-assistant placement proves which admitted Turn delegated the work. Carry
    // that identity through nested wakes and result-driven followups, including children that
    // outlive their parent. Missing provenance is never replaced with a time/current-binding
    // guess. UNION also makes duplicate events or repeated ancestry harmless.
    let mut query = connection
        .prepare(
            "WITH RECURSIVE owned_runs(instance_id, run_id, conversation_id) AS (
                SELECT json_extract(admission.snapshot_json, '$.instanceId'),
                       trace.run_id, trace.conversation_id
                FROM workflow_mail_runs admission
                JOIN conversation_turn_traces trace
                  ON trace.run_id = admission.run_id
                 AND trace.conversation_id = admission.conversation_id
                WHERE json_extract(admission.snapshot_json, '$.instanceId')
                      IN (SELECT value FROM json_each(?1))
                UNION
                SELECT parent.instance_id, child_trace.run_id, child_trace.conversation_id
                FROM owned_runs parent
                JOIN conversation_turn_traces parent_trace
                  ON parent_trace.run_id = parent.run_id
                 AND parent_trace.conversation_id = parent.conversation_id
                JOIN agent_collaboration_events event
                  ON event.activity_anchor_message_id = parent_trace.assistant_message_id
                 AND event.activity_parent_conversation_id = parent.conversation_id
                 AND event.kind = 'wake_created'
                 AND event.activity_semantic = 'started'
                JOIN agent_wake_requests wake
                  ON wake.source_agent_message_id = event.message_id
                 AND wake.agent_id = event.agent_id
                 AND wake.root_agent_id = event.root_agent_id
                 AND wake.requester_agent_id = event.activity_parent_agent_id
                JOIN agent_nodes child
                  ON child.agent_id = wake.agent_id
                 AND child.parent_agent_id = event.activity_parent_agent_id
                 AND child.root_agent_id = wake.root_agent_id
                JOIN conversation_turn_traces child_trace
                  ON child_trace.run_id = wake.run_id
                 AND child_trace.assistant_message_id = wake.assistant_message_id
                 AND child_trace.conversation_id = child.conversation_id
                UNION
                SELECT source.instance_id, target_trace.run_id, target_trace.conversation_id
                FROM owned_runs source
                JOIN conversation_turn_traces source_trace
                  ON source_trace.run_id = source.run_id
                 AND source_trace.conversation_id = source.conversation_id
                JOIN agent_wake_requests source_wake
                  ON source_wake.run_id = source.run_id
                 AND source_wake.assistant_message_id = source_trace.assistant_message_id
                JOIN agent_nodes sender
                  ON sender.agent_id = source_wake.agent_id
                 AND sender.conversation_id = source.conversation_id
                 AND sender.root_agent_id = source_wake.root_agent_id
                JOIN agent_mailbox_messages result
                  ON result.message_id = source_wake.result_message_id
                 AND result.kind = 'result'
                 AND result.sender_agent_id = sender.agent_id
                 AND result.recipient_agent_id = sender.parent_agent_id
                 AND result.root_agent_id = sender.root_agent_id
                JOIN agent_wake_requests target_wake
                  ON target_wake.source_agent_message_id = result.message_id
                 AND target_wake.agent_id = result.recipient_agent_id
                 AND target_wake.requester_agent_id = result.sender_agent_id
                 AND target_wake.root_agent_id = result.root_agent_id
                JOIN agent_nodes target
                  ON target.agent_id = target_wake.agent_id
                 AND target.root_agent_id = result.root_agent_id
                 AND target.parent_agent_id IS NOT NULL
                JOIN conversation_turn_traces target_trace
                  ON target_trace.run_id = target_wake.run_id
                 AND target_trace.assistant_message_id = target_wake.assistant_message_id
                 AND target_trace.conversation_id = target.conversation_id
            )
            SELECT owned.instance_id, trace.created_at, trace.completed_at
            FROM owned_runs owned
            JOIN conversation_turn_traces trace
              ON trace.run_id = owned.run_id AND trace.conversation_id = owned.conversation_id
            ORDER BY owned.instance_id, trace.created_at, trace.run_id",
        )
        .map_err(storage_error)?;
    // Read scalar times in one query for the entire list. Neither chat bodies nor full snapshots
    // are materialized, and there is no event-page limit that could truncate a long active span.
    let rows = query
        .query_map([requested], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Activity {
                    started_at: row.get(1)?,
                    completed_at: row.get(2)?,
                },
            ))
        })
        .map_err(storage_error)?;
    let mut result: HashMap<String, Activity> = HashMap::new();
    for row in rows {
        let (instance_id, next) = row.map_err(storage_error)?;
        result
            .entry(instance_id)
            .and_modify(|current| {
                if current
                    .completed_at
                    .is_some_and(|end| next.started_at > end)
                {
                    *current = next.clone();
                } else {
                    current.completed_at = current
                        .completed_at
                        .zip(next.completed_at)
                        .map(|(left, right)| left.max(right));
                }
            })
            .or_insert(next);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
