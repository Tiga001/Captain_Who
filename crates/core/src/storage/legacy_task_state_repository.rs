//! Read-only compatibility access for the retired Task Continuation State tables.
//!
//! New runtime code must not depend on this module. Old rows remain available for one
//! compatibility cycle so support tooling can inspect or export them without reintroducing them
//! into model context, Goal state, Todo state, compaction, or run lifecycle decisions.

use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyTaskControlStateRow {
    task_id: String,
    conversation_id: String,
    schema_version: i64,
    objective: String,
    source_message_id: String,
    status: String,
    revision: i64,
    current_run_id: Option<String>,
    stopped_reason: Option<String>,
    checkpoint_json: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyTaskControlStateRevisionRow {
    task_id: String,
    revision: i64,
    conversation_id: String,
    snapshot_json: String,
    mutation_kind: String,
    mutation_id: Option<String>,
    result_task_id: String,
    created_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyTaskStateExport {
    schema_version: u32,
    conversation_id: String,
    control_states: Vec<LegacyTaskControlStateRow>,
    revisions: Vec<LegacyTaskControlStateRevisionRow>,
}

pub(crate) fn export_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> Result<serde_json::Value, String> {
    let mut state_statement = connection
        .prepare(
            "SELECT task_id, conversation_id, schema_version, objective, source_message_id,
                    status, revision, current_run_id, stopped_reason, checkpoint_json,
                    created_at, updated_at
             FROM task_control_states
             WHERE conversation_id = ?1
             ORDER BY updated_at, task_id",
        )
        .map_err(database_error)?;
    let control_states = state_statement
        .query_map([conversation_id], |row| {
            Ok(LegacyTaskControlStateRow {
                task_id: row.get(0)?,
                conversation_id: row.get(1)?,
                schema_version: row.get(2)?,
                objective: row.get(3)?,
                source_message_id: row.get(4)?,
                status: row.get(5)?,
                revision: row.get(6)?,
                current_run_id: row.get(7)?,
                stopped_reason: row.get(8)?,
                checkpoint_json: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })
        .map_err(database_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(database_error)?;

    let mut revision_statement = connection
        .prepare(
            "SELECT task_id, revision, conversation_id, snapshot_json, mutation_kind,
                    mutation_id, result_task_id, created_at
             FROM task_control_state_revisions
             WHERE conversation_id = ?1
             ORDER BY task_id, revision",
        )
        .map_err(database_error)?;
    let revisions = revision_statement
        .query_map([conversation_id], |row| {
            Ok(LegacyTaskControlStateRevisionRow {
                task_id: row.get(0)?,
                revision: row.get(1)?,
                conversation_id: row.get(2)?,
                snapshot_json: row.get(3)?,
                mutation_kind: row.get(4)?,
                mutation_id: row.get(5)?,
                result_task_id: row.get(6)?,
                created_at: row.get(7)?,
            })
        })
        .map_err(database_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(database_error)?;

    serde_json::to_value(LegacyTaskStateExport {
        schema_version: 1,
        conversation_id: conversation_id.to_string(),
        control_states,
        revisions,
    })
    .map_err(|error| format!("无法序列化旧 Task State 诊断导出：{error}"))
}

fn database_error(error: rusqlite::Error) -> String {
    format!("旧 Task State 诊断读取失败：{error}")
}
