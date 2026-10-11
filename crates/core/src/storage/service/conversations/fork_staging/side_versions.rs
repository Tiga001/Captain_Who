//! Exact optimistic fences for copied side journals. Wall-clock timestamps are not revisions:
//! several writes may have the same millisecond, and MAX(updated_at, now) can preserve a value.
use rusqlite::{types::ValueRef, Connection};
use sha2::{Digest, Sha256};

const CHILD_SIDE_QUERIES: &[&str] = &[
    "SELECT * FROM agent_turn_diffs WHERE conversation_id=?1 ORDER BY assistant_message_id",
    "SELECT * FROM agent_turn_diff_files WHERE assistant_message_id IN (SELECT assistant_message_id FROM agent_turn_diffs WHERE conversation_id=?1) ORDER BY assistant_message_id,path",
    "SELECT * FROM agent_turn_diff_actions WHERE assistant_message_id IN (SELECT assistant_message_id FROM agent_turn_diffs WHERE conversation_id=?1) ORDER BY assistant_message_id,action_id",
    "SELECT * FROM managed_artifact_grants WHERE conversation_id=?1 ORDER BY artifact_id,run_id,call_id",
    "SELECT * FROM managed_artifacts WHERE artifact_id IN (SELECT artifact_id FROM managed_artifact_grants WHERE conversation_id=?1) ORDER BY artifact_id",
];

const FORK_SIDE_QUERIES: &[&str] = &[
    // Include the root roster so a concurrent sibling change cannot make a mixed tree snapshot.
    "SELECT * FROM agent_nodes WHERE conversation_id=?1 OR root_conversation_id=?1 ORDER BY agent_id",
    "SELECT * FROM conversation_world_state_epochs WHERE conversation_id=?1 ORDER BY generation,epoch_id",
    "SELECT * FROM conversation_world_state_records WHERE conversation_id=?1 ORDER BY journal_position",
    "SELECT * FROM agent_action_audit WHERE conversation_id=?1 ORDER BY action_id",
    "SELECT * FROM agent_file_changes WHERE conversation_id=?1 ORDER BY id",
    "SELECT * FROM agent_file_change_chunks WHERE transaction_id IN (SELECT id FROM agent_file_changes WHERE conversation_id=?1) ORDER BY transaction_id,mutation_index",
    "SELECT * FROM agent_file_change_operations WHERE transaction_id IN (SELECT id FROM agent_file_changes WHERE conversation_id=?1) ORDER BY transaction_id,mutation_index",
    "SELECT * FROM context_compaction_receipts WHERE conversation_id=?1 ORDER BY operation_id",
    "SELECT * FROM model_request_observations WHERE conversation_id=?1 ORDER BY id",
    "SELECT * FROM human_interaction_requests WHERE conversation_id=?1 ORDER BY sequence",
    "SELECT * FROM agent_run_guidances WHERE conversation_id=?1 ORDER BY guidance_id",
];

pub(super) fn side_versions(connection: &Connection, id: &str) -> Result<[u8; 32], String> {
    hash_queries(
        connection,
        id,
        FORK_SIDE_QUERIES
            .iter()
            .copied()
            .chain(CHILD_SIDE_QUERIES.iter().copied()),
    )
}

pub(in crate::storage::service) fn child_snapshot_side_version(
    connection: &Connection,
    id: &str,
) -> Result<[u8; 32], String> {
    hash_queries(connection, id, CHILD_SIDE_QUERIES.iter().copied())
}

fn hash_queries(
    connection: &Connection,
    id: &str,
    queries: impl Iterator<Item = &'static str>,
) -> Result<[u8; 32], String> {
    let mut digest = Sha256::new();
    for sql in queries {
        digest.update((sql.len() as u64).to_le_bytes());
        digest.update(sql.as_bytes());
        let mut statement = connection.prepare(sql).map_err(|error| error.to_string())?;
        let count = statement.column_count();
        let mut rows = statement.query([id]).map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            digest.update([1]);
            digest.update((count as u64).to_le_bytes());
            for index in 0..count {
                match row.get_ref(index).map_err(|error| error.to_string())? {
                    ValueRef::Null => digest.update([0]),
                    ValueRef::Integer(value) => {
                        digest.update([1]);
                        digest.update(value.to_le_bytes());
                    }
                    ValueRef::Real(value) => {
                        digest.update([2]);
                        digest.update(value.to_bits().to_le_bytes());
                    }
                    ValueRef::Text(value) => {
                        digest.update([3]);
                        digest.update((value.len() as u64).to_le_bytes());
                        // Borrow SQLite's row bytes; do not create a second JSON/body allocation.
                        digest.update(value);
                    }
                    ValueRef::Blob(value) => {
                        digest.update([4]);
                        digest.update((value.len() as u64).to_le_bytes());
                        digest.update(value);
                    }
                }
            }
        }
        digest.update([0]);
    }
    Ok(digest.finalize().into())
}
