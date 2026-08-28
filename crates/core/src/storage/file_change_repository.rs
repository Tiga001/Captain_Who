use crate::file_change::FileChangeMutationReceipt;
use crate::storage::models::{
    AgentFileChangeChunkRecord, AgentFileChangeOperationRecord, AgentFileChangeRecord,
};
use rusqlite::{params, Connection, OptionalExtension};

/// The only persisted staged FileChange schema supported by the canonical development database.
pub const AGENT_FILE_CHANGE_SCHEMA_VERSION: u32 = 1;
const EXPIRED_FILE_CHANGE_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub(crate) struct AgentFileChangeHistorySnapshot {
    pub chunks: Vec<AgentFileChangeChunkRecord>,
    pub operations: Vec<AgentFileChangeOperationRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentFileChangeProgressSaveOutcome {
    Applied,
    Idempotent(AgentFileChangeOperationRecord),
    ReplayMismatch,
    Conflict,
}

pub fn insert_file_change(
    connection: &Connection,
    change: &AgentFileChangeRecord,
) -> rusqlite::Result<()> {
    connection.execute(
        r#"
        INSERT INTO agent_file_changes (
            schema_version, id, conversation_id, project_id, run_id,
            source_tool_name, source_tool_call_id, source_tool_arguments_digest,
            permission_revision, tool_set_revision, provider_wire_revision,
            observation_id, observation_json, file_path, operation, strategy, status,
            base_revision, base_content, content, draft_revision, next_mutation_index,
            additions, deletions, line_count, byte_count, mutation_count, stats_final,
            summary, final_action_id, final_action_arguments_digest,
            final_permission_revision, final_tool_set_revision, final_provider_wire_revision,
            created_at, updated_at, expires_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
            ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30,
            ?31, ?32, ?33, ?34, ?35, ?36, ?37
        )
        "#,
        params![
            change.schema_version,
            change.id,
            change.conversation_id,
            change.project_id,
            change.run_id,
            change.source_tool_name,
            change.source_tool_call_id,
            change.source_tool_arguments_digest,
            change.permission_revision,
            change.tool_set_revision,
            change.provider_wire_revision,
            change.observation_id,
            change.observation_json,
            change.file_path,
            change.operation,
            change.strategy,
            change.status,
            change.base_revision,
            change.base_content,
            change.content,
            u64_to_i64(change.draft_revision)?,
            u64_to_i64(change.next_mutation_index)?,
            u64_to_i64(change.additions)?,
            u64_to_i64(change.deletions)?,
            u64_to_i64(change.line_count)?,
            u64_to_i64(change.byte_count)?,
            u64_to_i64(change.mutation_count)?,
            change.stats_final,
            change.summary,
            change.final_action_id,
            change.final_action_arguments_digest,
            change.final_permission_revision,
            change.final_tool_set_revision,
            change.final_provider_wire_revision,
            change.created_at,
            change.updated_at,
            change.expires_at,
        ],
    )?;
    Ok(())
}

pub(crate) fn load_file_change_history_snapshot(
    connection: &Connection,
    transaction_id: &str,
    cutoff_at: Option<i64>,
) -> rusqlite::Result<AgentFileChangeHistorySnapshot> {
    let mut chunks_statement = connection.prepare(
        r#"
        SELECT mutation_index, content_digest, byte_count, created_at
        FROM agent_file_change_chunks
        WHERE transaction_id = ?1 AND (?2 IS NULL OR created_at <= ?2)
        ORDER BY mutation_index ASC
        "#,
    )?;
    let chunks = chunks_statement
        .query_map(params![transaction_id, cutoff_at], |row| {
            Ok(AgentFileChangeChunkRecord {
                transaction_id: transaction_id.to_string(),
                mutation_index: i64_to_u64(row.get(0)?)?,
                content_digest: row.get(1)?,
                byte_count: i64_to_u64(row.get(2)?)?,
                created_at: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut operations_statement = connection.prepare(
        r#"
        SELECT mutation_index, source_tool_call_id, source_tool_arguments_digest,
               action, payload_digest, draft_revision, receipt_json, created_at
        FROM agent_file_change_operations
        WHERE transaction_id = ?1 AND (?2 IS NULL OR created_at <= ?2)
        ORDER BY mutation_index ASC
        "#,
    )?;
    let operations = operations_statement
        .query_map(params![transaction_id, cutoff_at], |row| {
            let operation = AgentFileChangeOperationRecord {
                transaction_id: transaction_id.to_string(),
                mutation_index: i64_to_u64(row.get(0)?)?,
                source_tool_call_id: row.get(1)?,
                source_tool_arguments_digest: row.get(2)?,
                action: row.get(3)?,
                payload_digest: row.get(4)?,
                draft_revision: i64_to_u64(row.get(5)?)?,
                receipt_json: row.get(6)?,
                created_at: row.get(7)?,
            };
            validate_operation_record(&operation)?;
            Ok(operation)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(AgentFileChangeHistorySnapshot { chunks, operations })
}

pub(crate) fn insert_file_change_history_snapshot(
    connection: &Connection,
    target_file_change_id: &str,
    snapshot: &AgentFileChangeHistorySnapshot,
) -> rusqlite::Result<()> {
    for chunk in &snapshot.chunks {
        connection.execute(
            r#"
            INSERT INTO agent_file_change_chunks (
                transaction_id, mutation_index, content_digest, byte_count, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                target_file_change_id,
                u64_to_i64(chunk.mutation_index)?,
                chunk.content_digest,
                u64_to_i64(chunk.byte_count)?,
                chunk.created_at,
            ],
        )?;
    }
    for operation in &snapshot.operations {
        validate_operation_record(operation)?;
        connection.execute(
            r#"
            INSERT INTO agent_file_change_operations (
                transaction_id, mutation_index, source_tool_call_id,
                source_tool_arguments_digest, action, payload_digest,
                draft_revision, receipt_json, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                target_file_change_id,
                u64_to_i64(operation.mutation_index)?,
                operation.source_tool_call_id,
                operation.source_tool_arguments_digest,
                operation.action,
                operation.payload_digest,
                u64_to_i64(operation.draft_revision)?,
                operation.receipt_json,
                operation.created_at,
            ],
        )?;
    }
    Ok(())
}

pub fn get_file_change(
    connection: &Connection,
    transaction_id: &str,
) -> rusqlite::Result<Option<AgentFileChangeRecord>> {
    connection
        .query_row(
            r#"
            SELECT schema_version, id, conversation_id, project_id, run_id,
                   source_tool_name, source_tool_call_id, source_tool_arguments_digest,
                   permission_revision, tool_set_revision, provider_wire_revision,
                   observation_id, observation_json, file_path, operation, strategy, status,
                   base_revision, base_content, content, draft_revision, next_mutation_index,
                   additions, deletions, line_count, byte_count, mutation_count, stats_final,
                   summary, final_action_id, final_action_arguments_digest,
                   final_permission_revision, final_tool_set_revision,
                   final_provider_wire_revision,
                   created_at, updated_at, expires_at
            FROM agent_file_changes
            WHERE id = ?1
            "#,
            [transaction_id],
            map_file_change,
        )
        .optional()
}

/// Loads one transaction only when every stable owner dimension matches exactly.
///
/// `project_id` deliberately uses SQLite `IS`: two absent projects match, while an absent project
/// never aliases a concrete project. The immutable begin Tool Call remains on the returned record;
/// append/edit/commit callers additionally bind their own exact call identity in the operation or
/// proposal they persist.
pub fn get_file_change_for_owner(
    connection: &Connection,
    transaction_id: &str,
    conversation_id: &str,
    project_id: Option<&str>,
    run_id: &str,
    source_tool_name: &str,
) -> rusqlite::Result<Option<AgentFileChangeRecord>> {
    connection
        .query_row(
            r#"
            SELECT schema_version, id, conversation_id, project_id, run_id,
                   source_tool_name, source_tool_call_id, source_tool_arguments_digest,
                   permission_revision, tool_set_revision, provider_wire_revision,
                   observation_id, observation_json, file_path, operation, strategy, status,
                   base_revision, base_content, content, draft_revision, next_mutation_index,
                   additions, deletions, line_count, byte_count, mutation_count, stats_final,
                   summary, final_action_id, final_action_arguments_digest,
                   final_permission_revision, final_tool_set_revision,
                   final_provider_wire_revision,
                   created_at, updated_at, expires_at
            FROM agent_file_changes
            WHERE id = ?1
              AND conversation_id = ?2
              AND project_id IS ?3
              AND run_id = ?4
              AND source_tool_name = ?5
            "#,
            params![
                transaction_id,
                conversation_id,
                project_id,
                run_id,
                source_tool_name
            ],
            map_file_change,
        )
        .optional()
}

/// Loads the one canonical transaction created by an exact `apply_patch` begin Tool Call.
///
/// The canonical schema makes `(run_id, source_tool_call_id)` unique. The remaining owner
/// dimensions are still matched here so a caller can never use the lookup as a cross-project or
/// cross-conversation transaction oracle.
pub fn get_file_change_for_source_call(
    connection: &Connection,
    conversation_id: &str,
    project_id: Option<&str>,
    run_id: &str,
    source_tool_call_id: &str,
) -> rusqlite::Result<Option<AgentFileChangeRecord>> {
    connection
        .query_row(
            r#"
            SELECT schema_version, id, conversation_id, project_id, run_id,
                   source_tool_name, source_tool_call_id, source_tool_arguments_digest,
                   permission_revision, tool_set_revision, provider_wire_revision,
                   observation_id, observation_json, file_path, operation, strategy, status,
                   base_revision, base_content, content, draft_revision, next_mutation_index,
                   additions, deletions, line_count, byte_count, mutation_count, stats_final,
                   summary, final_action_id, final_action_arguments_digest,
                   final_permission_revision, final_tool_set_revision,
                   final_provider_wire_revision,
                   created_at, updated_at, expires_at
            FROM agent_file_changes
            WHERE conversation_id = ?1
              AND project_id IS ?2
              AND run_id = ?3
              AND source_tool_name = 'apply_patch'
              AND source_tool_call_id = ?4
            "#,
            params![conversation_id, project_id, run_id, source_tool_call_id],
            map_file_change,
        )
        .optional()
}

pub fn list_file_changes_for_run(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Vec<AgentFileChangeRecord>> {
    let mut statement = connection.prepare(
        r#"
        SELECT schema_version, id, conversation_id, project_id, run_id,
               source_tool_name, source_tool_call_id, source_tool_arguments_digest,
               permission_revision, tool_set_revision, provider_wire_revision,
               observation_id, observation_json, file_path, operation, strategy, status,
               base_revision, base_content, content, draft_revision, next_mutation_index,
               additions, deletions, line_count, byte_count, mutation_count, stats_final,
               summary, final_action_id, final_action_arguments_digest,
               final_permission_revision, final_tool_set_revision,
               final_provider_wire_revision,
               created_at, updated_at, expires_at
        FROM agent_file_changes
        WHERE run_id = ?1
        ORDER BY created_at ASC, id ASC
        "#,
    )?;
    let changes = statement
        .query_map([run_id], map_file_change)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(changes)
}

pub fn settle_unresolved_file_changes_for_run(
    connection: &Connection,
    run_id: &str,
    status: &str,
    updated_at: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        r#"
        UPDATE agent_file_changes
        SET status = ?2, stats_final = 1, updated_at = ?3
        WHERE run_id = ?1
          AND status IN ('drafting', 'ready')
        "#,
        params![run_id, status, updated_at],
    )
}

pub fn expire_and_prune_file_changes(
    connection: &mut Connection,
    now: i64,
) -> rusqlite::Result<(usize, usize)> {
    let transaction = connection.transaction()?;
    let deleted = transaction.execute(
        r#"
        DELETE FROM agent_file_changes
        WHERE expires_at <= ?1
          AND status IN ('applied', 'rejected', 'conflict', 'failed', 'aborted', 'expired')
        "#,
        [now],
    )?;
    let expired = transaction.execute(
        r#"
        UPDATE agent_file_changes
        SET status = 'expired',
            stats_final = 1,
            updated_at = ?1,
            expires_at = ?2
        WHERE expires_at <= ?1
          AND status IN ('drafting', 'ready')
        "#,
        params![now, now.saturating_add(EXPIRED_FILE_CHANGE_RETENTION_MS)],
    )?;
    transaction.commit()?;
    Ok((expired, deleted))
}

pub fn save_file_change_progress(
    connection: &mut Connection,
    expected_draft_revision: u64,
    change: &AgentFileChangeRecord,
    chunk: Option<&AgentFileChangeChunkRecord>,
    operation: &AgentFileChangeOperationRecord,
) -> rusqlite::Result<AgentFileChangeProgressSaveOutcome> {
    let transaction = connection.transaction()?;
    validate_operation_record(operation)?;
    if let Some(existing) = get_operation(&transaction, &change.id, operation.mutation_index)? {
        let outcome = if existing.action == operation.action
            && existing.payload_digest == operation.payload_digest
        {
            AgentFileChangeProgressSaveOutcome::Idempotent(existing)
        } else {
            AgentFileChangeProgressSaveOutcome::ReplayMismatch
        };
        transaction.commit()?;
        return Ok(outcome);
    }
    let expected_next_revision = expected_draft_revision
        .checked_add(1)
        .ok_or_else(invalid_integer_error)?;
    if operation.transaction_id != change.id
        || operation.mutation_index != change.next_mutation_index.saturating_sub(1)
        || operation.draft_revision != expected_next_revision
        || change.draft_revision != expected_next_revision
        || change.next_mutation_index != operation.mutation_index.saturating_add(1)
        || change.mutation_count != change.next_mutation_index
        || chunk.is_some_and(|chunk| {
            chunk.transaction_id != change.id
                || chunk.mutation_index != operation.mutation_index
                || operation.action != "append"
        })
        || (operation.action == "append") != chunk.is_some()
    {
        return Ok(AgentFileChangeProgressSaveOutcome::Conflict);
    }
    let updated = update_file_change_cas(
        &transaction,
        expected_draft_revision,
        operation.mutation_index,
        change,
    )?;
    if !updated {
        transaction.commit()?;
        return Ok(AgentFileChangeProgressSaveOutcome::Conflict);
    }
    if let Some(chunk) = chunk {
        transaction.execute(
            r#"
            INSERT INTO agent_file_change_chunks (
                transaction_id, mutation_index, content_digest, byte_count, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                chunk.transaction_id,
                u64_to_i64(chunk.mutation_index)?,
                chunk.content_digest,
                u64_to_i64(chunk.byte_count)?,
                chunk.created_at,
            ],
        )?;
    }
    transaction.execute(
        r#"
        INSERT INTO agent_file_change_operations (
            transaction_id, mutation_index, source_tool_call_id,
            source_tool_arguments_digest, action, payload_digest,
            draft_revision, receipt_json, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        "#,
        params![
            operation.transaction_id,
            u64_to_i64(operation.mutation_index)?,
            operation.source_tool_call_id,
            operation.source_tool_arguments_digest,
            operation.action,
            operation.payload_digest,
            u64_to_i64(operation.draft_revision)?,
            operation.receipt_json,
            operation.created_at,
        ],
    )?;
    transaction.commit()?;
    Ok(AgentFileChangeProgressSaveOutcome::Applied)
}

pub fn update_file_change(
    connection: &Connection,
    change: &AgentFileChangeRecord,
) -> rusqlite::Result<()> {
    let updated = connection.execute(
        r#"
        UPDATE agent_file_changes SET
            status = ?2,
            base_revision = ?3,
            base_content = ?4,
            content = ?5,
            draft_revision = ?6,
            next_mutation_index = ?7,
            additions = ?8,
            deletions = ?9,
            line_count = ?10,
            byte_count = ?11,
            mutation_count = ?12,
            stats_final = ?13,
            summary = ?14,
            final_action_id = ?15,
            final_action_arguments_digest = ?16,
            final_permission_revision = ?17,
            final_tool_set_revision = ?18,
            final_provider_wire_revision = ?19,
            updated_at = ?20,
            expires_at = ?21
        WHERE id = ?1
          AND schema_version = ?22
          AND conversation_id = ?23
          AND project_id IS ?24
          AND run_id = ?25
          AND source_tool_name = ?26
          AND source_tool_call_id = ?27
          AND source_tool_arguments_digest = ?28
          AND permission_revision = ?29
          AND tool_set_revision = ?30
          AND provider_wire_revision = ?31
          AND observation_id IS ?32
          AND observation_json IS ?33
          AND file_path = ?34
          AND operation = ?35
          AND strategy IS ?36
        "#,
        params![
            change.id,
            change.status,
            change.base_revision,
            change.base_content,
            change.content,
            u64_to_i64(change.draft_revision)?,
            u64_to_i64(change.next_mutation_index)?,
            u64_to_i64(change.additions)?,
            u64_to_i64(change.deletions)?,
            u64_to_i64(change.line_count)?,
            u64_to_i64(change.byte_count)?,
            u64_to_i64(change.mutation_count)?,
            change.stats_final,
            change.summary,
            change.final_action_id,
            change.final_action_arguments_digest,
            change.final_permission_revision,
            change.final_tool_set_revision,
            change.final_provider_wire_revision,
            change.updated_at,
            change.expires_at,
            change.schema_version,
            change.conversation_id,
            change.project_id,
            change.run_id,
            change.source_tool_name,
            change.source_tool_call_id,
            change.source_tool_arguments_digest,
            change.permission_revision,
            change.tool_set_revision,
            change.provider_wire_revision,
            change.observation_id,
            change.observation_json,
            change.file_path,
            change.operation,
            change.strategy,
        ],
    )?;
    if updated == 1 {
        Ok(())
    } else {
        Err(rusqlite::Error::QueryReturnedNoRows)
    }
}

/// Applies one status transition without allowing a concurrent mutation or terminal transition to
/// be overwritten. Callers must supply the exact state they observed before producing approval or
/// abort side effects.
pub fn transition_file_change(
    connection: &Connection,
    expected_status: &str,
    expected_draft_revision: u64,
    expected_next_mutation_index: u64,
    change: &AgentFileChangeRecord,
) -> rusqlite::Result<bool> {
    let updated = connection.execute(
        r#"
        UPDATE agent_file_changes SET
            status = ?2, stats_final = ?3, summary = ?4, final_action_id = ?5,
            final_action_arguments_digest = ?6, final_permission_revision = ?7,
            final_tool_set_revision = ?8, final_provider_wire_revision = ?9,
            updated_at = ?10, expires_at = ?11
        WHERE id = ?1
          AND status = ?12
          AND draft_revision = ?13
          AND next_mutation_index = ?14
          AND conversation_id = ?15
          AND project_id IS ?16
          AND run_id = ?17
          AND source_tool_name = ?18
          AND source_tool_call_id = ?19
          AND source_tool_arguments_digest = ?20
          AND permission_revision = ?21
          AND tool_set_revision = ?22
          AND provider_wire_revision = ?23
        "#,
        params![
            change.id,
            change.status,
            change.stats_final,
            change.summary,
            change.final_action_id,
            change.final_action_arguments_digest,
            change.final_permission_revision,
            change.final_tool_set_revision,
            change.final_provider_wire_revision,
            change.updated_at,
            change.expires_at,
            expected_status,
            u64_to_i64(expected_draft_revision)?,
            u64_to_i64(expected_next_mutation_index)?,
            change.conversation_id,
            change.project_id,
            change.run_id,
            change.source_tool_name,
            change.source_tool_call_id,
            change.source_tool_arguments_digest,
            change.permission_revision,
            change.tool_set_revision,
            change.provider_wire_revision,
        ],
    )?;
    Ok(updated == 1)
}

pub fn get_operation(
    connection: &Connection,
    transaction_id: &str,
    mutation_index: u64,
) -> rusqlite::Result<Option<AgentFileChangeOperationRecord>> {
    connection
        .query_row(
            r#"
            SELECT source_tool_call_id, source_tool_arguments_digest, action, payload_digest,
                   draft_revision, receipt_json, created_at
            FROM agent_file_change_operations
            WHERE transaction_id = ?1 AND mutation_index = ?2
            "#,
            params![transaction_id, u64_to_i64(mutation_index)?],
            |row| {
                let operation = AgentFileChangeOperationRecord {
                    transaction_id: transaction_id.to_string(),
                    mutation_index,
                    source_tool_call_id: row.get(0)?,
                    source_tool_arguments_digest: row.get(1)?,
                    action: row.get(2)?,
                    payload_digest: row.get(3)?,
                    draft_revision: i64_to_u64(row.get(4)?)?,
                    receipt_json: row.get(5)?,
                    created_at: row.get(6)?,
                };
                validate_operation_record(&operation)?;
                Ok(operation)
            },
        )
        .optional()
}

fn update_file_change_cas(
    connection: &Connection,
    expected_draft_revision: u64,
    expected_mutation_index: u64,
    change: &AgentFileChangeRecord,
) -> rusqlite::Result<bool> {
    let updated = connection.execute(
        r#"
        UPDATE agent_file_changes SET
            status = ?2, content = ?3, draft_revision = ?4, next_mutation_index = ?5,
            additions = ?6, deletions = ?7, line_count = ?8, byte_count = ?9,
            mutation_count = ?10, stats_final = ?11, summary = ?12,
            updated_at = ?13, expires_at = ?14
        WHERE id = ?1
          AND draft_revision = ?15
          AND next_mutation_index = ?16
          AND status IN ('drafting', 'ready')
          AND conversation_id = ?17
          AND project_id IS ?18
          AND run_id = ?19
          AND source_tool_name = ?20
          AND source_tool_call_id = ?21
          AND source_tool_arguments_digest = ?22
          AND permission_revision = ?23
          AND tool_set_revision = ?24
          AND provider_wire_revision = ?25
        "#,
        params![
            change.id,
            change.status,
            change.content,
            u64_to_i64(change.draft_revision)?,
            u64_to_i64(change.next_mutation_index)?,
            u64_to_i64(change.additions)?,
            u64_to_i64(change.deletions)?,
            u64_to_i64(change.line_count)?,
            u64_to_i64(change.byte_count)?,
            u64_to_i64(change.mutation_count)?,
            change.stats_final,
            change.summary,
            change.updated_at,
            change.expires_at,
            u64_to_i64(expected_draft_revision)?,
            u64_to_i64(expected_mutation_index)?,
            change.conversation_id,
            change.project_id,
            change.run_id,
            change.source_tool_name,
            change.source_tool_call_id,
            change.source_tool_arguments_digest,
            change.permission_revision,
            change.tool_set_revision,
            change.provider_wire_revision,
        ],
    )?;
    Ok(updated == 1)
}

fn map_file_change(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentFileChangeRecord> {
    Ok(AgentFileChangeRecord {
        schema_version: i64_to_u32(row.get(0)?)?,
        id: row.get(1)?,
        conversation_id: row.get(2)?,
        project_id: row.get(3)?,
        run_id: row.get(4)?,
        source_tool_name: row.get(5)?,
        source_tool_call_id: row.get(6)?,
        source_tool_arguments_digest: row.get(7)?,
        permission_revision: row.get(8)?,
        tool_set_revision: row.get(9)?,
        provider_wire_revision: row.get(10)?,
        observation_id: row.get(11)?,
        observation_json: row.get(12)?,
        file_path: row.get(13)?,
        operation: row.get(14)?,
        strategy: row.get(15)?,
        status: row.get(16)?,
        base_revision: row.get(17)?,
        base_content: row.get(18)?,
        content: row.get(19)?,
        draft_revision: i64_to_u64(row.get(20)?)?,
        next_mutation_index: i64_to_u64(row.get(21)?)?,
        additions: i64_to_u64(row.get(22)?)?,
        deletions: i64_to_u64(row.get(23)?)?,
        line_count: i64_to_u64(row.get(24)?)?,
        byte_count: i64_to_u64(row.get(25)?)?,
        mutation_count: i64_to_u64(row.get(26)?)?,
        stats_final: row.get(27)?,
        summary: row.get(28)?,
        final_action_id: row.get(29)?,
        final_action_arguments_digest: row.get(30)?,
        final_permission_revision: row.get(31)?,
        final_tool_set_revision: row.get(32)?,
        final_provider_wire_revision: row.get(33)?,
        created_at: row.get(34)?,
        updated_at: row.get(35)?,
        expires_at: row.get(36)?,
    })
}

fn u64_to_i64(value: u64) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|_| invalid_integer_error())
}

fn i64_to_u64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| invalid_integer_error())
}

fn i64_to_u32(value: i64) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|_| invalid_integer_error())
}

fn invalid_integer_error() -> rusqlite::Error {
    rusqlite::Error::IntegralValueOutOfRange(0, i64::MAX)
}

fn validate_operation_record(operation: &AgentFileChangeOperationRecord) -> rusqlite::Result<()> {
    let receipt = serde_json::from_str::<FileChangeMutationReceipt>(&operation.receipt_json)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    receipt
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if operation.transaction_id.trim().is_empty()
        || operation.source_tool_call_id.trim().is_empty()
        || operation.source_tool_arguments_digest.trim().is_empty()
        || !matches!(operation.action.as_str(), "append" | "edit")
        || operation.payload_digest.trim().is_empty()
        || receipt.transaction_id != operation.transaction_id
        || receipt.index != operation.mutation_index
        || receipt.draft_revision != operation.draft_revision
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::run_migrations;

    fn record() -> AgentFileChangeRecord {
        AgentFileChangeRecord {
            schema_version: AGENT_FILE_CHANGE_SCHEMA_VERSION,
            id: "file-change-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            project_id: Some("project-1".to_string()),
            run_id: "run-1".to_string(),
            source_tool_name: "apply_patch".to_string(),
            source_tool_call_id: "call-begin-1".to_string(),
            source_tool_arguments_digest: "digest-begin-1".to_string(),
            permission_revision: "permission-1".to_string(),
            tool_set_revision: "tool-set-1".to_string(),
            provider_wire_revision: "provider-protocol-v1".to_string(),
            observation_id: "fobs_1".to_string(),
            observation_json: r#"{"schemaVersion":1}"#.to_string(),
            file_path: "report.md".to_string(),
            operation: "create".to_string(),
            strategy: None,
            status: "drafting".to_string(),
            base_revision: None,
            base_content: String::new(),
            content: String::new(),
            draft_revision: 0,
            next_mutation_index: 0,
            additions: 0,
            deletions: 0,
            line_count: 0,
            byte_count: 0,
            mutation_count: 0,
            stats_final: false,
            summary: None,
            final_action_id: None,
            final_action_arguments_digest: None,
            final_permission_revision: None,
            final_tool_set_revision: None,
            final_provider_wire_revision: None,
            created_at: 1,
            updated_at: 1,
            expires_at: 10,
        }
    }

    fn freeze_final_identity(change: &mut AgentFileChangeRecord, suffix: &str) {
        change.stats_final = true;
        change.final_action_id = Some(format!("action-{suffix}"));
        change.final_action_arguments_digest = Some(format!("action-digest-{suffix}"));
        change.final_permission_revision = Some(change.permission_revision.clone());
        change.final_tool_set_revision = Some(change.tool_set_revision.clone());
        change.final_provider_wire_revision = Some(change.provider_wire_revision.clone());
    }

    fn setup() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (id, project_id, title, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params!["conversation-1", "project-1", "Test", 1_i64, 1_i64],
            )
            .unwrap();
        connection
    }

    fn append_records(
        change: &AgentFileChangeRecord,
        payload_digest: &str,
    ) -> (AgentFileChangeChunkRecord, AgentFileChangeOperationRecord) {
        (
            AgentFileChangeChunkRecord {
                transaction_id: change.id.clone(),
                mutation_index: 0,
                content_digest: "content-1".to_string(),
                byte_count: 6,
                created_at: 2,
            },
            AgentFileChangeOperationRecord {
                transaction_id: change.id.clone(),
                mutation_index: 0,
                source_tool_call_id: "call-append-1".to_string(),
                source_tool_arguments_digest: "digest-append-1".to_string(),
                action: "append".to_string(),
                payload_digest: payload_digest.to_string(),
                draft_revision: 1,
                receipt_json: serde_json::json!({
                    "schemaVersion": 1,
                    "transactionId": change.id,
                    "index": 0,
                    "draftRevision": 1,
                    "nextIndex": 1,
                    "byteCount": 6,
                    "lineCount": 1,
                    "allowedNextActions": ["append", "edit", "commit", "status", "abort"],
                    "requiresCommitBeforeResponse": true
                })
                .to_string(),
                created_at: 2,
            },
        )
    }

    #[test]
    fn persists_progress_with_one_parent_and_operation_transaction() {
        let mut connection = setup();
        let mut change = record();
        insert_file_change(&connection, &change).unwrap();
        change.content = "hello\n".to_string();
        change.additions = 1;
        change.line_count = 1;
        change.byte_count = 6;
        change.draft_revision = 1;
        change.next_mutation_index = 1;
        change.mutation_count = 1;
        let (chunk, operation) = append_records(&change, "payload-1");
        assert_eq!(
            save_file_change_progress(&mut connection, 0, &change, Some(&chunk), &operation)
                .unwrap(),
            AgentFileChangeProgressSaveOutcome::Applied
        );

        let restored = get_file_change(&connection, "file-change-1")
            .unwrap()
            .unwrap();
        assert_eq!(restored.content, "hello\n");
        assert_eq!(restored.draft_revision, 1);
        assert_eq!(restored.next_mutation_index, 1);
        assert_eq!(
            get_operation(&connection, "file-change-1", 0)
                .unwrap()
                .unwrap(),
            operation
        );
    }

    #[test]
    fn progress_replay_is_digest_idempotent_and_mismatch_is_closed() {
        let mut connection = setup();
        let mut change = record();
        insert_file_change(&connection, &change).unwrap();
        change.content = "hello\n".to_string();
        change.draft_revision = 1;
        change.next_mutation_index = 1;
        change.mutation_count = 1;
        change.byte_count = 6;
        let (chunk, operation) = append_records(&change, "payload-1");
        save_file_change_progress(&mut connection, 0, &change, Some(&chunk), &operation).unwrap();

        let mut replay = operation.clone();
        replay.source_tool_call_id = "call-retry".to_string();
        replay.created_at = 99;
        assert!(matches!(
            save_file_change_progress(&mut connection, 0, &change, Some(&chunk), &replay)
                .unwrap(),
            AgentFileChangeProgressSaveOutcome::Idempotent(existing)
                if existing == operation
        ));
        replay.payload_digest = "different".to_string();
        assert_eq!(
            save_file_change_progress(&mut connection, 0, &change, Some(&chunk), &replay).unwrap(),
            AgentFileChangeProgressSaveOutcome::ReplayMismatch
        );
    }

    #[test]
    fn stale_revision_and_out_of_order_index_do_not_mutate_parent() {
        let mut connection = setup();
        let mut change = record();
        insert_file_change(&connection, &change).unwrap();
        change.content = "hello\n".to_string();
        change.draft_revision = 1;
        change.next_mutation_index = 2;
        change.mutation_count = 2;
        let (mut chunk, mut operation) = append_records(&change, "payload-2");
        chunk.mutation_index = 1;
        operation.mutation_index = 1;
        operation.draft_revision = 2;
        operation.receipt_json = serde_json::json!({
            "schemaVersion": 1,
            "transactionId": change.id,
            "index": 1,
            "draftRevision": 2,
            "nextIndex": 2,
            "byteCount": 6,
            "lineCount": 1,
            "allowedNextActions": ["append", "edit", "commit", "status", "abort"],
            "requiresCommitBeforeResponse": true
        })
        .to_string();
        assert_eq!(
            save_file_change_progress(&mut connection, 0, &change, Some(&chunk), &operation)
                .unwrap(),
            AgentFileChangeProgressSaveOutcome::Conflict
        );
        assert_eq!(
            get_file_change(&connection, "file-change-1")
                .unwrap()
                .unwrap()
                .draft_revision,
            0
        );
    }

    #[test]
    fn owner_lookup_requires_conversation_project_run_and_tool() {
        let connection = setup();
        insert_file_change(&connection, &record()).unwrap();
        assert!(get_file_change_for_owner(
            &connection,
            "file-change-1",
            "conversation-1",
            Some("project-1"),
            "run-1",
            "apply_patch"
        )
        .unwrap()
        .is_some());
        for (conversation, project, run, tool) in [
            (
                "conversation-other",
                Some("project-1"),
                "run-1",
                "apply_patch",
            ),
            ("conversation-1", None, "run-1", "apply_patch"),
            (
                "conversation-1",
                Some("project-other"),
                "run-1",
                "apply_patch",
            ),
            (
                "conversation-1",
                Some("project-1"),
                "run-other",
                "apply_patch",
            ),
            ("conversation-1", Some("project-1"), "run-1", "write_file"),
        ] {
            assert!(get_file_change_for_owner(
                &connection,
                "file-change-1",
                conversation,
                project,
                run,
                tool
            )
            .unwrap()
            .is_none());
        }
    }

    #[test]
    fn status_transition_is_revision_and_status_cas_guarded() {
        let connection = setup();
        let mut change = record();
        insert_file_change(&connection, &change).unwrap();
        change.status = "aborted".to_string();
        change.stats_final = true;
        change.updated_at = 2;
        assert!(transition_file_change(&connection, "drafting", 0, 0, &change).unwrap());
        assert_eq!(
            get_file_change(&connection, &change.id)
                .unwrap()
                .unwrap()
                .status,
            "aborted"
        );
        assert!(!transition_file_change(&connection, "drafting", 0, 0, &change).unwrap());

        let mut stale = record();
        stale.id = "file-change-stale".to_string();
        stale.source_tool_call_id = "call-begin-stale".to_string();
        insert_file_change(&connection, &stale).unwrap();
        stale.status = "ready".to_string();
        assert!(!transition_file_change(&connection, "drafting", 1, 0, &stale).unwrap());
        assert_eq!(
            get_file_change(&connection, &stale.id)
                .unwrap()
                .unwrap()
                .status,
            "drafting"
        );
    }

    #[test]
    fn lists_and_settles_only_unresolved_changes_for_a_run() {
        let connection = setup();
        let mut unresolved = record();
        insert_file_change(&connection, &unresolved).unwrap();
        let mut applied = record();
        applied.id = "file-change-2".to_string();
        applied.source_tool_call_id = "call-begin-2".to_string();
        applied.status = "applied".to_string();
        applied.created_at = 2;
        freeze_final_identity(&mut applied, "applied");
        insert_file_change(&connection, &applied).unwrap();
        let mut applying = record();
        applying.id = "file-change-applying".to_string();
        applying.source_tool_call_id = "call-begin-applying".to_string();
        applying.status = "applying".to_string();
        applying.created_at = 2;
        freeze_final_identity(&mut applying, "applying");
        insert_file_change(&connection, &applying).unwrap();
        let mut other_run = record();
        other_run.id = "file-change-3".to_string();
        other_run.run_id = "run-2".to_string();
        other_run.created_at = 3;
        insert_file_change(&connection, &other_run).unwrap();

        let listed = list_file_changes_for_run(&connection, "run-1").unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|change| change.id.as_str())
                .collect::<Vec<_>>(),
            vec!["file-change-1", "file-change-2", "file-change-applying"]
        );
        assert_eq!(
            settle_unresolved_file_changes_for_run(&connection, "run-1", "failed", 9).unwrap(),
            1
        );
        unresolved = get_file_change(&connection, "file-change-1")
            .unwrap()
            .unwrap();
        assert_eq!(unresolved.status, "failed");
        assert!(unresolved.stats_final);
        assert_eq!(
            get_file_change(&connection, "file-change-2")
                .unwrap()
                .unwrap()
                .status,
            "applied"
        );
        assert_eq!(
            get_file_change(&connection, "file-change-applying")
                .unwrap()
                .unwrap()
                .status,
            "applying",
            "commit-unknown transactions must remain available for reconciliation"
        );
        assert_eq!(
            get_file_change(&connection, "file-change-3")
                .unwrap()
                .unwrap()
                .status,
            "drafting"
        );
    }

    #[test]
    fn expires_unresolved_changes_and_prunes_terminal_changes() {
        let mut connection = setup();

        let mut unresolved = record();
        unresolved.expires_at = 10;
        insert_file_change(&connection, &unresolved).unwrap();
        let mut terminal = record();
        terminal.id = "file-change-terminal".to_string();
        terminal.source_tool_call_id = "call-begin-terminal".to_string();
        terminal.status = "applied".to_string();
        terminal.expires_at = 10;
        freeze_final_identity(&mut terminal, "terminal");
        insert_file_change(&connection, &terminal).unwrap();

        let (expired, deleted) = expire_and_prune_file_changes(&mut connection, 20).unwrap();

        assert_eq!((expired, deleted), (1, 1));
        let unresolved = get_file_change(&connection, "file-change-1")
            .unwrap()
            .unwrap();
        assert_eq!(unresolved.status, "expired");
        assert!(unresolved.expires_at > 20);
        assert!(get_file_change(&connection, "file-change-terminal")
            .unwrap()
            .is_none());
    }
}
