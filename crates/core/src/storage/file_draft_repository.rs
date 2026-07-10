use crate::storage::models::{
    AgentFileDraftChunkRecord, AgentFileDraftOperationRecord, AgentFileDraftRecord,
};
use rusqlite::{params, Connection, OptionalExtension};

pub fn insert_draft(connection: &Connection, draft: &AgentFileDraftRecord) -> rusqlite::Result<()> {
    connection.execute(
        r#"
        INSERT INTO agent_file_drafts (
            id, conversation_id, project_id, run_id, file_path, mode, status,
            base_revision, base_content, content, additions, deletions, line_count,
            byte_count, chunk_count, next_chunk_index, stats_final, summary,
            final_action_id, created_at, updated_at, expires_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
        )
        "#,
        params![
            draft.id,
            draft.conversation_id,
            draft.project_id,
            draft.run_id,
            draft.file_path,
            draft.mode,
            draft.status,
            draft.base_revision,
            draft.base_content,
            draft.content,
            draft.additions as i64,
            draft.deletions as i64,
            draft.line_count as i64,
            draft.byte_count as i64,
            draft.chunk_count as i64,
            draft.next_chunk_index as i64,
            draft.stats_final,
            draft.summary,
            draft.final_action_id,
            draft.created_at,
            draft.updated_at,
            draft.expires_at,
        ],
    )?;
    Ok(())
}

pub fn get_draft(
    connection: &Connection,
    draft_id: &str,
) -> rusqlite::Result<Option<AgentFileDraftRecord>> {
    connection
        .query_row(
            r#"
            SELECT id, conversation_id, project_id, run_id, file_path, mode, status,
                   base_revision, base_content, content, additions, deletions, line_count,
                   byte_count, chunk_count, next_chunk_index, stats_final, summary,
                   final_action_id, created_at, updated_at, expires_at
            FROM agent_file_drafts
            WHERE id = ?1
            "#,
            [draft_id],
            map_draft,
        )
        .optional()
}

pub fn list_drafts_for_run(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Vec<AgentFileDraftRecord>> {
    let mut statement = connection.prepare(
        r#"
        SELECT id, conversation_id, project_id, run_id, file_path, mode, status,
               base_revision, base_content, content, additions, deletions, line_count,
               byte_count, chunk_count, next_chunk_index, stats_final, summary,
               final_action_id, created_at, updated_at, expires_at
        FROM agent_file_drafts
        WHERE run_id = ?1
        ORDER BY created_at ASC, id ASC
        "#,
    )?;
    let drafts = statement
        .query_map([run_id], map_draft)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(drafts)
}

pub fn settle_unresolved_drafts_for_run(
    connection: &Connection,
    run_id: &str,
    status: &str,
    updated_at: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        r#"
        UPDATE agent_file_drafts
        SET status = ?2, stats_final = 1, updated_at = ?3
        WHERE run_id = ?1
          AND status IN ('drafting', 'ready', 'waiting_approval', 'applying')
        "#,
        params![run_id, status, updated_at],
    )
}

pub fn save_draft_progress(
    connection: &mut Connection,
    draft: &AgentFileDraftRecord,
    chunk: Option<&AgentFileDraftChunkRecord>,
    operation: Option<&AgentFileDraftOperationRecord>,
) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    update_draft(&transaction, draft)?;
    if let Some(chunk) = chunk {
        transaction.execute(
            r#"
            INSERT INTO agent_file_draft_chunks (
                draft_id, chunk_index, content_hash, byte_count, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(draft_id, chunk_index) DO NOTHING
            "#,
            params![
                chunk.draft_id,
                chunk.chunk_index as i64,
                chunk.content_hash,
                chunk.byte_count as i64,
                chunk.created_at,
            ],
        )?;
    }
    if let Some(operation) = operation {
        transaction.execute(
            r#"
            INSERT INTO agent_file_draft_operations (
                draft_id, sequence, operation, payload_hash, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(draft_id, sequence) DO NOTHING
            "#,
            params![
                operation.draft_id,
                operation.sequence as i64,
                operation.operation,
                operation.payload_hash,
                operation.created_at,
            ],
        )?;
    }
    transaction.commit()
}

pub fn update_draft(connection: &Connection, draft: &AgentFileDraftRecord) -> rusqlite::Result<()> {
    connection.execute(
        r#"
        UPDATE agent_file_drafts SET
            status = ?2,
            base_revision = ?3,
            base_content = ?4,
            content = ?5,
            additions = ?6,
            deletions = ?7,
            line_count = ?8,
            byte_count = ?9,
            chunk_count = ?10,
            next_chunk_index = ?11,
            stats_final = ?12,
            summary = ?13,
            final_action_id = ?14,
            updated_at = ?15,
            expires_at = ?16
        WHERE id = ?1
        "#,
        params![
            draft.id,
            draft.status,
            draft.base_revision,
            draft.base_content,
            draft.content,
            draft.additions as i64,
            draft.deletions as i64,
            draft.line_count as i64,
            draft.byte_count as i64,
            draft.chunk_count as i64,
            draft.next_chunk_index as i64,
            draft.stats_final,
            draft.summary,
            draft.final_action_id,
            draft.updated_at,
            draft.expires_at,
        ],
    )?;
    Ok(())
}

pub fn get_chunk_hash(
    connection: &Connection,
    draft_id: &str,
    chunk_index: u64,
) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            r#"
            SELECT content_hash
            FROM agent_file_draft_chunks
            WHERE draft_id = ?1 AND chunk_index = ?2
            "#,
            params![draft_id, chunk_index as i64],
            |row| row.get(0),
        )
        .optional()
}

pub fn next_operation_sequence(connection: &Connection, draft_id: &str) -> rusqlite::Result<u64> {
    let next = connection.query_row(
        r#"
        SELECT COALESCE(MAX(sequence), -1) + 1
        FROM agent_file_draft_operations
        WHERE draft_id = ?1
        "#,
        [draft_id],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(next.max(0) as u64)
}

fn map_draft(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentFileDraftRecord> {
    Ok(AgentFileDraftRecord {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        project_id: row.get(2)?,
        run_id: row.get(3)?,
        file_path: row.get(4)?,
        mode: row.get(5)?,
        status: row.get(6)?,
        base_revision: row.get(7)?,
        base_content: row.get(8)?,
        content: row.get(9)?,
        additions: row.get::<_, i64>(10)?.max(0) as u64,
        deletions: row.get::<_, i64>(11)?.max(0) as u64,
        line_count: row.get::<_, i64>(12)?.max(0) as u64,
        byte_count: row.get::<_, i64>(13)?.max(0) as u64,
        chunk_count: row.get::<_, i64>(14)?.max(0) as u64,
        next_chunk_index: row.get::<_, i64>(15)?.max(0) as u64,
        stats_final: row.get(16)?,
        summary: row.get(17)?,
        final_action_id: row.get(18)?,
        created_at: row.get(19)?,
        updated_at: row.get(20)?,
        expires_at: row.get(21)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::run_migrations;

    fn record() -> AgentFileDraftRecord {
        AgentFileDraftRecord {
            id: "draft-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            project_id: None,
            run_id: "run-1".to_string(),
            file_path: "report.md".to_string(),
            mode: "create".to_string(),
            status: "drafting".to_string(),
            base_revision: None,
            base_content: String::new(),
            content: String::new(),
            additions: 0,
            deletions: 0,
            line_count: 0,
            byte_count: 0,
            chunk_count: 0,
            next_chunk_index: 0,
            stats_final: false,
            summary: None,
            final_action_id: None,
            created_at: 1,
            updated_at: 1,
            expires_at: 10,
        }
    }

    #[test]
    fn persists_draft_progress_and_chunk_identity() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params!["conversation-1", "Test", 1_i64, 1_i64],
            )
            .unwrap();
        let mut draft = record();
        insert_draft(&connection, &draft).unwrap();
        draft.content = "hello\n".to_string();
        draft.additions = 1;
        draft.line_count = 1;
        draft.byte_count = 6;
        draft.chunk_count = 1;
        draft.next_chunk_index = 1;
        let chunk = AgentFileDraftChunkRecord {
            draft_id: draft.id.clone(),
            chunk_index: 0,
            content_hash: "hash-1".to_string(),
            byte_count: 6,
            created_at: 2,
        };
        save_draft_progress(&mut connection, &draft, Some(&chunk), None).unwrap();

        let restored = get_draft(&connection, "draft-1").unwrap().unwrap();
        assert_eq!(restored.content, "hello\n");
        assert_eq!(restored.next_chunk_index, 1);
        assert_eq!(
            get_chunk_hash(&connection, "draft-1", 0)
                .unwrap()
                .as_deref(),
            Some("hash-1")
        );
    }

    #[test]
    fn lists_and_settles_only_unresolved_drafts_for_a_run() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params!["conversation-1", "Test", 1_i64, 1_i64],
            )
            .unwrap();
        let mut unresolved = record();
        insert_draft(&connection, &unresolved).unwrap();
        let mut applied = record();
        applied.id = "draft-2".to_string();
        applied.status = "applied".to_string();
        applied.created_at = 2;
        insert_draft(&connection, &applied).unwrap();
        let mut other_run = record();
        other_run.id = "draft-3".to_string();
        other_run.run_id = "run-2".to_string();
        other_run.created_at = 3;
        insert_draft(&connection, &other_run).unwrap();

        let listed = list_drafts_for_run(&connection, "run-1").unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|draft| draft.id.as_str())
                .collect::<Vec<_>>(),
            vec!["draft-1", "draft-2"]
        );
        assert_eq!(
            settle_unresolved_drafts_for_run(&connection, "run-1", "failed", 9).unwrap(),
            1
        );
        unresolved = get_draft(&connection, "draft-1").unwrap().unwrap();
        assert_eq!(unresolved.status, "failed");
        assert!(unresolved.stats_final);
        assert_eq!(
            get_draft(&connection, "draft-2").unwrap().unwrap().status,
            "applied"
        );
        assert_eq!(
            get_draft(&connection, "draft-3").unwrap().unwrap().status,
            "drafting"
        );
    }
}
