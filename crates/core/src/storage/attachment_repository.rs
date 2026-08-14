use crate::storage::models::AttachmentRecord;
use rusqlite::{params, Connection};

pub fn save_attachment(
    connection: &Connection,
    attachment: &AttachmentRecord,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO attachments (
            id,
            conversation_id,
            message_id,
            project_id,
            kind,
            original_name,
            mime_type,
            size_bytes,
            storage_rel_path,
            created_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(id) DO UPDATE SET
            conversation_id = excluded.conversation_id,
            message_id = excluded.message_id,
            project_id = excluded.project_id,
            kind = excluded.kind,
            original_name = excluded.original_name,
            mime_type = excluded.mime_type,
            size_bytes = excluded.size_bytes,
            storage_rel_path = excluded.storage_rel_path,
            created_at = excluded.created_at
        ",
        params![
            &attachment.id,
            &attachment.conversation_id,
            &attachment.message_id,
            &attachment.project_id,
            &attachment.kind,
            &attachment.original_name,
            &attachment.mime_type,
            attachment.size_bytes,
            &attachment.storage_rel_path,
            attachment.created_at
        ],
    )?;

    Ok(())
}

pub fn insert_attachment(
    connection: &Connection,
    attachment: &AttachmentRecord,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO attachments (
            id,
            conversation_id,
            message_id,
            project_id,
            kind,
            original_name,
            mime_type,
            size_bytes,
            storage_rel_path,
            created_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ",
        params![
            &attachment.id,
            &attachment.conversation_id,
            &attachment.message_id,
            &attachment.project_id,
            &attachment.kind,
            &attachment.original_name,
            &attachment.mime_type,
            attachment.size_bytes,
            &attachment.storage_rel_path,
            attachment.created_at
        ],
    )?;
    Ok(())
}

pub fn list_conversation_attachments(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<AttachmentRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            id,
            conversation_id,
            message_id,
            project_id,
            kind,
            original_name,
            mime_type,
            size_bytes,
            storage_rel_path,
            created_at
        FROM attachments
        WHERE conversation_id = ?1
        ORDER BY created_at ASC, id ASC
        ",
    )?;

    let attachments = statement
        .query_map(params![conversation_id], attachment_from_row)?
        .collect();

    attachments
}

pub fn list_project_attachments_excluding_conversation(
    connection: &Connection,
    project_id: &str,
    excluded_conversation_id: &str,
) -> rusqlite::Result<Vec<AttachmentRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            id,
            conversation_id,
            message_id,
            project_id,
            kind,
            original_name,
            mime_type,
            size_bytes,
            storage_rel_path,
            created_at
        FROM attachments
        WHERE project_id = ?1
            AND conversation_id != ?2
        ORDER BY created_at DESC, id ASC
        ",
    )?;

    let attachments = statement
        .query_map(
            params![project_id, excluded_conversation_id],
            attachment_from_row,
        )?
        .collect();

    attachments
}

pub fn list_conversation_attachments_for_library(
    connection: &Connection,
    conversation_id: &str,
    admitted_run_id: Option<&str>,
) -> rusqlite::Result<Vec<AttachmentRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            attachment.id,
            attachment.conversation_id,
            attachment.message_id,
            attachment.project_id,
            attachment.kind,
            attachment.original_name,
            attachment.mime_type,
            attachment.size_bytes,
            attachment.storage_rel_path,
            attachment.created_at
        FROM attachments AS attachment
        WHERE attachment.conversation_id = ?1
          AND NOT EXISTS (
            SELECT 1
            FROM conversation_turn_rewrites AS rewrite
            WHERE rewrite.conversation_id = attachment.conversation_id
              AND (
                  rewrite.source_user_message_id = attachment.message_id
                  OR rewrite.source_assistant_message_id = attachment.message_id
              )
          )
          AND (
            NOT EXISTS (
                SELECT 1
                FROM agent_run_guidance_attachments AS ownership
                WHERE ownership.attachment_id = attachment.id
            )
            OR EXISTS (
                SELECT 1
                FROM agent_run_guidance_attachments AS ownership
                JOIN agent_run_guidances AS guidance
                  ON guidance.guidance_id = ownership.guidance_id
                WHERE ownership.attachment_id = attachment.id
                  AND (
                    guidance.status = 'applied'
                    OR (
                        ?2 IS NOT NULL
                        AND guidance.run_id = ?2
                        AND guidance.status = 'queued'
                    )
                  )
            )
          )
        ORDER BY attachment.created_at ASC, attachment.id ASC
        ",
    )?;
    let attachments = statement
        .query_map(
            params![conversation_id, admitted_run_id],
            attachment_from_row,
        )?
        .collect();
    attachments
}

pub fn list_project_attachments_for_library_excluding_conversation(
    connection: &Connection,
    project_id: &str,
    excluded_conversation_id: &str,
) -> rusqlite::Result<Vec<AttachmentRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            attachment.id,
            attachment.conversation_id,
            attachment.message_id,
            attachment.project_id,
            attachment.kind,
            attachment.original_name,
            attachment.mime_type,
            attachment.size_bytes,
            attachment.storage_rel_path,
            attachment.created_at
        FROM attachments AS attachment
        WHERE attachment.project_id = ?1
          AND attachment.conversation_id != ?2
          AND NOT EXISTS (
            SELECT 1
            FROM conversation_turn_rewrites AS rewrite
            WHERE rewrite.conversation_id = attachment.conversation_id
              AND (
                  rewrite.source_user_message_id = attachment.message_id
                  OR rewrite.source_assistant_message_id = attachment.message_id
              )
          )
          AND (
            NOT EXISTS (
                SELECT 1
                FROM agent_run_guidance_attachments AS ownership
                WHERE ownership.attachment_id = attachment.id
            )
            OR EXISTS (
                SELECT 1
                FROM agent_run_guidance_attachments AS ownership
                JOIN agent_run_guidances AS guidance
                  ON guidance.guidance_id = ownership.guidance_id
                WHERE ownership.attachment_id = attachment.id
                  AND guidance.status = 'applied'
            )
          )
        ORDER BY attachment.created_at DESC, attachment.id ASC
        ",
    )?;
    let attachments = statement
        .query_map(
            params![project_id, excluded_conversation_id],
            attachment_from_row,
        )?
        .collect();
    attachments
}

pub fn list_ordinary_conversation_attachments(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<AttachmentRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            attachment.id,
            attachment.conversation_id,
            attachment.message_id,
            attachment.project_id,
            attachment.kind,
            attachment.original_name,
            attachment.mime_type,
            attachment.size_bytes,
            attachment.storage_rel_path,
            attachment.created_at
        FROM attachments AS attachment
        WHERE attachment.conversation_id = ?1
          AND NOT EXISTS (
            SELECT 1
            FROM agent_run_guidance_attachments AS ownership
            WHERE ownership.attachment_id = attachment.id
          )
        ORDER BY attachment.created_at ASC, attachment.id ASC
        ",
    )?;
    let attachments = statement
        .query_map([conversation_id], attachment_from_row)?
        .collect();
    attachments
}

pub fn list_project_deletion_attachments(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<Vec<AttachmentRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            id,
            conversation_id,
            message_id,
            project_id,
            kind,
            original_name,
            mime_type,
            size_bytes,
            storage_rel_path,
            created_at
        FROM attachments
        WHERE project_id = ?1
            OR conversation_id IN (
                SELECT id
                FROM conversations
                WHERE project_id = ?1
            )
        ORDER BY created_at ASC, id ASC
        ",
    )?;

    let attachments = statement
        .query_map(params![project_id], attachment_from_row)?
        .collect();

    attachments
}

pub fn list_attachment_storage_rel_paths(connection: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "
        SELECT storage_rel_path
        FROM attachments
        ",
    )?;

    let paths = statement.query_map([], |row| row.get(0))?.collect();

    paths
}

pub fn get_attachment(
    connection: &Connection,
    attachment_id: &str,
) -> rusqlite::Result<Option<AttachmentRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            id,
            conversation_id,
            message_id,
            project_id,
            kind,
            original_name,
            mime_type,
            size_bytes,
            storage_rel_path,
            created_at
        FROM attachments
        WHERE id = ?1
        LIMIT 1
        ",
    )?;

    let mut rows = statement.query(params![attachment_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };

    attachment_from_row(row).map(Some)
}

pub fn list_message_attachments(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<Vec<AttachmentRecord>> {
    let mut attachments = Vec::new();

    for message_id in message_ids {
        let mut statement = connection.prepare(
            "
            SELECT
                id,
                conversation_id,
                message_id,
                project_id,
                kind,
                original_name,
                mime_type,
                size_bytes,
                storage_rel_path,
                created_at
            FROM attachments
            WHERE conversation_id = ?1 AND message_id = ?2
            ORDER BY created_at ASC, id ASC
            ",
        )?;

        let mut message_attachments = statement
            .query_map(params![conversation_id, message_id], attachment_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        attachments.append(&mut message_attachments);
    }

    Ok(attachments)
}

pub fn list_message_attachments_for_fork(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<Vec<AttachmentRecord>> {
    let mut attachments = Vec::new();

    for message_id in message_ids {
        let mut statement = connection.prepare(
            "
            SELECT
                attachment.id,
                attachment.conversation_id,
                attachment.message_id,
                attachment.project_id,
                attachment.kind,
                attachment.original_name,
                attachment.mime_type,
                attachment.size_bytes,
                attachment.storage_rel_path,
                attachment.created_at
            FROM attachments AS attachment
            WHERE attachment.conversation_id = ?1
              AND attachment.message_id = ?2
              AND (
                NOT EXISTS (
                    SELECT 1
                    FROM agent_run_guidance_attachments AS ownership
                    WHERE ownership.attachment_id = attachment.id
                )
                OR EXISTS (
                    SELECT 1
                    FROM agent_run_guidance_attachments AS ownership
                    JOIN agent_run_guidances AS guidance
                      ON guidance.guidance_id = ownership.guidance_id
                    WHERE ownership.attachment_id = attachment.id
                      AND guidance.status = 'applied'
                )
              )
            ORDER BY attachment.created_at ASC, attachment.id ASC
            ",
        )?;

        let mut message_attachments = statement
            .query_map(params![conversation_id, message_id], attachment_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        attachments.append(&mut message_attachments);
    }

    Ok(attachments)
}

pub fn delete_message_attachments(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    for message_id in message_ids {
        connection.execute(
            "DELETE FROM attachments WHERE conversation_id = ?1 AND message_id = ?2",
            params![conversation_id, message_id],
        )?;
    }

    Ok(())
}

fn attachment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentRecord> {
    Ok(AttachmentRecord {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        message_id: row.get(2)?,
        project_id: row.get(3)?,
        kind: row.get(4)?,
        original_name: row.get(5)?,
        mime_type: row.get(6)?,
        size_bytes: row.get(7)?,
        storage_rel_path: row.get(8)?,
        created_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_attachment_query_excludes_current_conversation() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE attachments (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    message_id TEXT NOT NULL,
                    project_id TEXT,
                    kind TEXT NOT NULL,
                    original_name TEXT NOT NULL,
                    mime_type TEXT,
                    size_bytes INTEGER NOT NULL,
                    storage_rel_path TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                ",
            )
            .unwrap();

        save_attachment(
            &connection,
            &attachment("current", "conversation-current", "project-1", 10),
        )
        .unwrap();
        save_attachment(
            &connection,
            &attachment("other", "conversation-other", "project-1", 20),
        )
        .unwrap();
        save_attachment(
            &connection,
            &attachment("other-project", "conversation-3", "project-2", 30),
        )
        .unwrap();

        let attachments = list_project_attachments_excluding_conversation(
            &connection,
            "project-1",
            "conversation-current",
        )
        .unwrap();

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].id, "other");
        assert_eq!(attachments[0].conversation_id, "conversation-other");
    }

    fn attachment(
        id: &str,
        conversation_id: &str,
        project_id: &str,
        created_at: i64,
    ) -> AttachmentRecord {
        AttachmentRecord {
            id: id.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: format!("message-{id}"),
            project_id: Some(project_id.to_string()),
            kind: "file".to_string(),
            original_name: format!("{id}.txt"),
            mime_type: Some("text/plain".to_string()),
            size_bytes: 10,
            storage_rel_path: format!("{id}.txt"),
            created_at,
        }
    }
}
