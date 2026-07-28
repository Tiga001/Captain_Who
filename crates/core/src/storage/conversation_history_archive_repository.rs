//! Lossless, conversation-scoped storage for sanitized tool-result observations.
//!
//! Durable conversation traces intentionally retain bounded projections. This archive stores the
//! security-sanitized textual result before those length limits are applied, compressed in
//! independently readable UTF-8 chunks.

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::io::{Error as IoError, ErrorKind};
use uuid::Uuid;

const ARCHIVE_REF_PREFIX: &str = "history-archive-";
const CONTENT_HASH_PREFIX: &str = "sha256:";
const CHUNK_TARGET_BYTES: usize = 256 * 1024;
const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Clone)]
pub struct ConversationHistoryArchiveInput {
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub sequence: u64,
    pub call_id: String,
    pub tool: String,
    pub content_type: String,
    pub content: String,
    pub truncated_at_source: bool,
    pub model_projection_truncated: bool,
    pub archive_projection_truncated: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationHistoryArchiveDescriptor {
    pub archive_ref: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub sequence: u64,
    pub call_id: String,
    pub tool: String,
    pub content_type: String,
    pub content_hash: String,
    pub total_bytes: u64,
    pub total_chars: u64,
    pub chunk_count: u64,
    pub compression: String,
    pub truncated_at_source: bool,
    /// Complete persistence of the payload supplied to this archive, not source completeness.
    ///
    /// A descriptor may validly have both `archived_completely` and `truncated_at_source` set:
    /// the backend then preserved all bytes it received while the upstream source omitted some.
    pub archived_completely: bool,
    pub model_projection_truncated: bool,
    pub archive_projection_truncated: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationHistoryArchivePageUnit {
    Char,
    Byte,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationHistoryArchivePage {
    pub descriptor: ConversationHistoryArchiveDescriptor,
    pub unit: ConversationHistoryArchivePageUnit,
    pub start: u64,
    pub end: u64,
    pub content: String,
    pub truncated: bool,
    pub next_cursor: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ConversationHistoryArchiveForkCopy {
    pub source_archive_ref: String,
    pub target_archive_ref: String,
    pub target_conversation_id: String,
    pub target_assistant_message_id: String,
    pub target_call_id: String,
}

pub fn store_archive(
    connection: &mut Connection,
    input: &ConversationHistoryArchiveInput,
) -> rusqlite::Result<ConversationHistoryArchiveDescriptor> {
    validate_input(input)?;
    let content_bytes = input.content.as_bytes();
    let content_hash = content_hash(content_bytes);
    if let Some(existing) = find_archive_for_trace_item(
        connection,
        &input.conversation_id,
        &input.assistant_message_id,
        input.sequence,
    )? {
        if existing.call_id != input.call_id
            || existing.tool != input.tool
            || existing.content_type != input.content_type
            || existing.content_hash != content_hash
            || existing.total_bytes != content_bytes.len() as u64
            || existing.truncated_at_source != input.truncated_at_source
            || existing.model_projection_truncated != input.model_projection_truncated
            || existing.archive_projection_truncated != input.archive_projection_truncated
        {
            return Err(invalid_data(
                "history archive identity was reused for different tool-result content",
            ));
        }
        index_archive_content(connection, &existing, &input.content)?;
        return Ok(existing);
    }

    let archive_ref = format!("{ARCHIVE_REF_PREFIX}{}", Uuid::new_v4());
    let chunks = split_utf8_chunks(&input.content);
    let transaction = connection.transaction()?;
    transaction.execute(
        "
        INSERT INTO conversation_history_blobs (
            archive_ref, conversation_id, assistant_message_id, sequence,
            call_id, tool, content_type, content_hash,
            uncompressed_bytes, uncompressed_chars, chunk_count, compression,
            truncated_at_source, archived_completely,
            model_projection_truncated, archive_projection_truncated, created_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
            ?9, ?10, ?11, 'zstd', ?12, 1, ?13, ?14, ?15
        )
        ",
        params![
            &archive_ref,
            &input.conversation_id,
            &input.assistant_message_id,
            sqlite_integer(input.sequence)?,
            &input.call_id,
            &input.tool,
            &input.content_type,
            &content_hash,
            sqlite_integer(content_bytes.len() as u64)?,
            sqlite_integer(input.content.chars().count() as u64)?,
            sqlite_integer(chunks.len() as u64)?,
            input.truncated_at_source,
            input.model_projection_truncated,
            input.archive_projection_truncated,
            input.created_at,
        ],
    )?;

    let mut byte_offset = 0_u64;
    let mut char_offset = 0_u64;
    for (index, chunk) in chunks.iter().enumerate() {
        let compressed = zstd::stream::encode_all(chunk.as_bytes(), ZSTD_LEVEL)
            .map_err(|error| invalid_data(format!("cannot compress history archive: {error}")))?;
        let chunk_bytes = chunk.len() as u64;
        let chunk_chars = chunk.chars().count() as u64;
        transaction.execute(
            "
            INSERT INTO conversation_history_blob_chunks (
                archive_ref, chunk_index,
                uncompressed_start_byte, uncompressed_start_char,
                uncompressed_bytes, uncompressed_chars, compressed_bytes, payload
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ",
            params![
                &archive_ref,
                sqlite_integer(index as u64)?,
                sqlite_integer(byte_offset)?,
                sqlite_integer(char_offset)?,
                sqlite_integer(chunk_bytes)?,
                sqlite_integer(chunk_chars)?,
                sqlite_integer(compressed.len() as u64)?,
                compressed,
            ],
        )?;
        byte_offset += chunk_bytes;
        char_offset += chunk_chars;
    }
    let descriptor = ConversationHistoryArchiveDescriptor {
        archive_ref: archive_ref.clone(),
        conversation_id: input.conversation_id.clone(),
        assistant_message_id: input.assistant_message_id.clone(),
        sequence: input.sequence,
        call_id: input.call_id.clone(),
        tool: input.tool.clone(),
        content_type: input.content_type.clone(),
        content_hash: content_hash.clone(),
        total_bytes: content_bytes.len() as u64,
        total_chars: input.content.chars().count() as u64,
        chunk_count: chunks.len() as u64,
        compression: "zstd".to_string(),
        truncated_at_source: input.truncated_at_source,
        archived_completely: true,
        model_projection_truncated: input.model_projection_truncated,
        archive_projection_truncated: input.archive_projection_truncated,
        created_at: input.created_at,
    };
    index_archive_content(&transaction, &descriptor, &input.content)?;
    transaction.commit()?;

    find_archive_by_ref(connection, &input.conversation_id, &archive_ref)?
        .ok_or_else(|| invalid_data("stored history archive cannot be reloaded"))
}

pub fn find_archive_for_trace_item(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
    sequence: u64,
) -> rusqlite::Result<Option<ConversationHistoryArchiveDescriptor>> {
    connection
        .query_row(
            &format!(
                "{} WHERE conversation_id = ?1 AND assistant_message_id = ?2 AND sequence = ?3",
                descriptor_select()
            ),
            params![
                conversation_id,
                assistant_message_id,
                sqlite_integer(sequence)?
            ],
            descriptor_from_row,
        )
        .optional()
}

pub fn find_archive_by_ref(
    connection: &Connection,
    conversation_id: &str,
    archive_ref: &str,
) -> rusqlite::Result<Option<ConversationHistoryArchiveDescriptor>> {
    connection
        .query_row(
            &format!(
                "{} WHERE conversation_id = ?1 AND archive_ref = ?2",
                descriptor_select()
            ),
            params![conversation_id, archive_ref],
            descriptor_from_row,
        )
        .optional()
}

pub fn read_archive_page(
    connection: &Connection,
    conversation_id: &str,
    archive_ref: &str,
    unit: ConversationHistoryArchivePageUnit,
    start: u64,
    maximum: u64,
) -> rusqlite::Result<Option<ConversationHistoryArchivePage>> {
    if maximum == 0 {
        return Err(invalid_data("history archive page size must be positive"));
    }
    let Some(descriptor) = find_archive_by_ref(connection, conversation_id, archive_ref)? else {
        return Ok(None);
    };
    let total = match unit {
        ConversationHistoryArchivePageUnit::Char => descriptor.total_chars,
        ConversationHistoryArchivePageUnit::Byte => descriptor.total_bytes,
    };
    if start > total {
        return Err(invalid_data(format!(
            "history archive page starts after the end: {start} > {total}"
        )));
    }
    let requested_end = start.saturating_add(maximum).min(total);
    let mut statement = connection.prepare(
        "
        SELECT uncompressed_start_byte, uncompressed_start_char,
               uncompressed_bytes, uncompressed_chars, payload
        FROM conversation_history_blob_chunks
        WHERE archive_ref = ?1
        ORDER BY chunk_index ASC
        ",
    )?;
    let rows = statement.query_map([archive_ref], |row| {
        Ok(StoredChunk {
            start_byte: row.get::<_, u64>(0)?,
            start_char: row.get::<_, u64>(1)?,
            bytes: row.get::<_, u64>(2)?,
            chars: row.get::<_, u64>(3)?,
            payload: row.get::<_, Vec<u8>>(4)?,
        })
    })?;

    let mut content = String::new();
    for row in rows {
        let chunk = row?;
        let (chunk_start, chunk_len) = match unit {
            ConversationHistoryArchivePageUnit::Char => (chunk.start_char, chunk.chars),
            ConversationHistoryArchivePageUnit::Byte => (chunk.start_byte, chunk.bytes),
        };
        let chunk_end = chunk_start.saturating_add(chunk_len);
        if chunk_end <= start || chunk_start >= requested_end {
            continue;
        }
        let decoded = zstd::stream::decode_all(chunk.payload.as_slice())
            .map_err(|error| invalid_data(format!("cannot decompress history archive: {error}")))?;
        let decoded = String::from_utf8(decoded)
            .map_err(|error| invalid_data(format!("history archive is not UTF-8: {error}")))?;
        let local_start = start.saturating_sub(chunk_start);
        let local_end = requested_end.min(chunk_end).saturating_sub(chunk_start);
        match unit {
            ConversationHistoryArchivePageUnit::Char => {
                content.extend(
                    decoded
                        .chars()
                        .skip(local_start as usize)
                        .take(local_end.saturating_sub(local_start) as usize),
                );
            }
            ConversationHistoryArchivePageUnit::Byte => {
                let local_start = local_start as usize;
                let mut local_end = local_end as usize;
                if !decoded.is_char_boundary(local_start) {
                    return Err(invalid_data(
                        "history archive byte cursor must begin on a UTF-8 boundary",
                    ));
                }
                while local_end > local_start && !decoded.is_char_boundary(local_end) {
                    local_end -= 1;
                }
                if local_end == local_start && chunk_start + (local_start as u64) < total {
                    local_end = decoded[local_start..]
                        .chars()
                        .next()
                        .map(|character| local_start + character.len_utf8())
                        .unwrap_or(local_start);
                }
                let slice = decoded.get(local_start..local_end).ok_or_else(|| {
                    invalid_data(
                        "history archive byte cursor must begin and end on UTF-8 boundaries",
                    )
                })?;
                content.push_str(slice);
            }
        }
    }

    let actual_units = match unit {
        ConversationHistoryArchivePageUnit::Char => content.chars().count() as u64,
        ConversationHistoryArchivePageUnit::Byte => content.len() as u64,
    };
    let end = start.saturating_add(actual_units);
    let truncated = end < total;
    Ok(Some(ConversationHistoryArchivePage {
        descriptor,
        unit,
        start,
        end,
        content,
        truncated,
        next_cursor: truncated.then_some(end),
    }))
}

/// Finds a case-insensitive text occurrence in one exact-history archive.
///
/// The returned offset is measured in Unicode scalar values so it can be passed directly to
/// `read_archive_page` with `ConversationHistoryArchivePageUnit::Char`. Exact history remains the
/// source of truth; the FTS index is used only to select the archive candidate.
pub fn find_archive_match_char_offset(
    connection: &Connection,
    conversation_id: &str,
    archive_ref: &str,
    query: &str,
) -> rusqlite::Result<Option<u64>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(invalid_data("history archive match query cannot be empty"));
    }
    let Some(descriptor) = find_archive_by_ref(connection, conversation_id, archive_ref)? else {
        return Ok(None);
    };
    let content = read_complete_archive_content(connection, &descriptor)?;
    Ok(find_case_insensitive_char_offset(&content, query))
}

pub(crate) fn load_fork_copy(
    connection: &Connection,
    source_conversation_id: &str,
    source_archive_ref: &str,
    target_conversation_id: &str,
    target_assistant_message_id: &str,
    target_call_id: &str,
) -> rusqlite::Result<ConversationHistoryArchiveForkCopy> {
    find_archive_by_ref(connection, source_conversation_id, source_archive_ref)?
        .ok_or_else(|| invalid_data("fork trace references a missing history archive"))?;
    Ok(ConversationHistoryArchiveForkCopy {
        source_archive_ref: source_archive_ref.to_string(),
        target_archive_ref: format!("{ARCHIVE_REF_PREFIX}{}", Uuid::new_v4()),
        target_conversation_id: target_conversation_id.to_string(),
        target_assistant_message_id: target_assistant_message_id.to_string(),
        target_call_id: target_call_id.to_string(),
    })
}

pub(crate) fn clone_archive_in_connection(
    connection: &Connection,
    copy: &ConversationHistoryArchiveForkCopy,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO conversation_history_blobs (
            archive_ref, conversation_id, assistant_message_id, sequence,
            call_id, tool, content_type, content_hash,
            uncompressed_bytes, uncompressed_chars, chunk_count, compression,
            truncated_at_source, archived_completely,
            model_projection_truncated, archive_projection_truncated, created_at
        )
        SELECT
            ?1, ?2, ?3, sequence,
            ?4, tool, content_type, content_hash,
            uncompressed_bytes, uncompressed_chars, chunk_count, compression,
            truncated_at_source, archived_completely,
            model_projection_truncated, archive_projection_truncated, created_at
        FROM conversation_history_blobs
        WHERE archive_ref = ?5
        ",
        params![
            &copy.target_archive_ref,
            &copy.target_conversation_id,
            &copy.target_assistant_message_id,
            &copy.target_call_id,
            &copy.source_archive_ref,
        ],
    )?;
    connection.execute(
        "
        INSERT INTO conversation_history_blob_chunks (
            archive_ref, chunk_index,
            uncompressed_start_byte, uncompressed_start_char,
            uncompressed_bytes, uncompressed_chars, compressed_bytes, payload
        )
        SELECT
            ?1, chunk_index,
            uncompressed_start_byte, uncompressed_start_char,
            uncompressed_bytes, uncompressed_chars, compressed_bytes, payload
        FROM conversation_history_blob_chunks
        WHERE archive_ref = ?2
        ORDER BY chunk_index ASC
        ",
        params![&copy.target_archive_ref, &copy.source_archive_ref],
    )?;
    connection.execute(
        "
        INSERT INTO conversation_history_fts (
            ref_key, conversation_id, record_type, item_kind, message_id,
            assistant_message_id, sequence, archive_ref, call_id, tool,
            status, run_id, created_at, position, within_message_order, content
        )
        SELECT
            'archive:' || ?1,
            ?2,
            'archive',
            'tool_result_archive',
            NULL,
            ?3,
            source.sequence,
            ?1,
            ?4,
            source.tool,
            indexed.status,
            target_trace.run_id,
            source.created_at,
            target_message.position,
            source.sequence + 1,
            indexed.content
        FROM conversation_history_blobs AS source
        INNER JOIN conversation_history_fts AS indexed
            ON indexed.ref_key = 'archive:' || source.archive_ref
        INNER JOIN messages AS target_message
            ON target_message.id = ?3
        LEFT JOIN conversation_turn_traces AS target_trace
            ON target_trace.assistant_message_id = ?3
        WHERE source.archive_ref = ?5
        ",
        params![
            &copy.target_archive_ref,
            &copy.target_conversation_id,
            &copy.target_assistant_message_id,
            &copy.target_call_id,
            &copy.source_archive_ref,
        ],
    )?;
    Ok(())
}

pub(crate) fn backfill_history_search_index(connection: &Connection) -> rusqlite::Result<()> {
    let archive_refs = {
        let mut statement = connection.prepare(
            "SELECT archive_ref, conversation_id
             FROM conversation_history_blobs AS archive
             WHERE NOT EXISTS (
                SELECT 1 FROM conversation_history_fts
                WHERE ref_key = 'archive:' || archive.archive_ref
             )
             ORDER BY created_at ASC, archive_ref ASC",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (archive_ref, conversation_id) in archive_refs {
        let descriptor = find_archive_by_ref(connection, &conversation_id, &archive_ref)?
            .ok_or_else(|| invalid_data("history archive disappeared during FTS backfill"))?;
        let content = read_complete_archive_content(connection, &descriptor)?;
        index_archive_content(connection, &descriptor, &content)?;
    }
    Ok(())
}

fn index_archive_content(
    connection: &Connection,
    descriptor: &ConversationHistoryArchiveDescriptor,
    content: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM conversation_history_fts WHERE ref_key = ?1",
        [format!("archive:{}", descriptor.archive_ref)],
    )?;
    connection.execute(
        "
        INSERT INTO conversation_history_fts (
            ref_key, conversation_id, record_type, item_kind, message_id,
            assistant_message_id, sequence, archive_ref, call_id, tool,
            status, run_id, created_at, position, within_message_order, content
        ) VALUES (
            ?1, ?2, 'archive', 'tool_result_archive', NULL,
            ?3, ?4, ?5, ?6, ?7,
            (
                SELECT json_extract(item_json, '$.status')
                FROM conversation_turn_trace_items
                WHERE assistant_message_id = ?3 AND sequence = ?4
            ),
            (
                SELECT run_id FROM conversation_turn_traces
                WHERE assistant_message_id = ?3
            ),
            ?8,
            (
                SELECT position FROM messages
                WHERE id = ?3 AND conversation_id = ?2
            ),
            ?4 + 1,
            ?9
        )
        ",
        params![
            format!("archive:{}", descriptor.archive_ref),
            &descriptor.conversation_id,
            &descriptor.assistant_message_id,
            sqlite_integer(descriptor.sequence)?,
            &descriptor.archive_ref,
            &descriptor.call_id,
            &descriptor.tool,
            descriptor.created_at,
            content,
        ],
    )?;
    Ok(())
}

fn read_complete_archive_content(
    connection: &Connection,
    descriptor: &ConversationHistoryArchiveDescriptor,
) -> rusqlite::Result<String> {
    let chunks = {
        let mut statement = connection.prepare(
            "SELECT payload
             FROM conversation_history_blob_chunks
             WHERE archive_ref = ?1
             ORDER BY chunk_index ASC",
        )?;
        let rows = statement
            .query_map([&descriptor.archive_ref], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    if chunks.len() as u64 != descriptor.chunk_count {
        return Err(invalid_data(
            "history archive chunk count changed during FTS indexing",
        ));
    }
    let mut bytes = Vec::with_capacity(descriptor.total_bytes as usize);
    for payload in chunks {
        let decoded = zstd::stream::decode_all(payload.as_slice())
            .map_err(|error| invalid_data(format!("cannot decompress history archive: {error}")))?;
        bytes.extend_from_slice(&decoded);
    }
    if bytes.len() as u64 != descriptor.total_bytes
        || content_hash(&bytes) != descriptor.content_hash
    {
        return Err(invalid_data(
            "history archive failed exact hash validation during FTS indexing",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| invalid_data(format!("history archive is not UTF-8: {error}")))
}

fn descriptor_select() -> &'static str {
    "
    SELECT archive_ref, conversation_id, assistant_message_id, sequence,
           call_id, tool, content_type, content_hash,
           uncompressed_bytes, uncompressed_chars, chunk_count, compression,
           truncated_at_source, archived_completely,
           model_projection_truncated, archive_projection_truncated, created_at
    FROM conversation_history_blobs
    "
}

fn descriptor_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ConversationHistoryArchiveDescriptor> {
    Ok(ConversationHistoryArchiveDescriptor {
        archive_ref: row.get(0)?,
        conversation_id: row.get(1)?,
        assistant_message_id: row.get(2)?,
        sequence: row.get(3)?,
        call_id: row.get(4)?,
        tool: row.get(5)?,
        content_type: row.get(6)?,
        content_hash: row.get(7)?,
        total_bytes: row.get(8)?,
        total_chars: row.get(9)?,
        chunk_count: row.get(10)?,
        compression: row.get(11)?,
        truncated_at_source: row.get(12)?,
        archived_completely: row.get(13)?,
        model_projection_truncated: row.get(14)?,
        archive_projection_truncated: row.get(15)?,
        created_at: row.get(16)?,
    })
}

fn validate_input(input: &ConversationHistoryArchiveInput) -> rusqlite::Result<()> {
    for (label, value) in [
        ("conversation id", input.conversation_id.as_str()),
        ("assistant message id", input.assistant_message_id.as_str()),
        ("call id", input.call_id.as_str()),
        ("tool", input.tool.as_str()),
        ("content type", input.content_type.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(invalid_data(format!("history archive {label} is empty")));
        }
    }
    if input.created_at < 0 {
        return Err(invalid_data(
            "history archive created_at must be non-negative",
        ));
    }
    Ok(())
}

fn split_utf8_chunks(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return vec![""];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < content.len() {
        let mut end = (start + CHUNK_TARGET_BYTES).min(content.len());
        while end > start && !content.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = content[start..]
                .char_indices()
                .nth(1)
                .map(|(offset, _)| start + offset)
                .unwrap_or(content.len());
        }
        chunks.push(&content[start..end]);
        start = end;
    }
    chunks
}

fn content_hash(content: &[u8]) -> String {
    format!("{CONTENT_HASH_PREFIX}{:x}", Sha256::digest(content))
}

fn find_case_insensitive_char_offset(content: &str, query: &str) -> Option<u64> {
    if let Some(offset) = content.find(query) {
        return Some(content[..offset].chars().count() as u64);
    }
    let folded_query = query.to_lowercase();
    if folded_query.is_empty() {
        return None;
    }
    let folded_content = content.to_lowercase();
    let folded_byte_offset = folded_content.find(&folded_query)?;
    let mut current_folded_byte = 0_usize;
    for (char_offset, character) in content.chars().enumerate() {
        if current_folded_byte == folded_byte_offset {
            return Some(char_offset as u64);
        }
        current_folded_byte = current_folded_byte
            .saturating_add(character.to_lowercase().map(char::len_utf8).sum::<usize>());
        if current_folded_byte > folded_byte_offset {
            return None;
        }
    }
    None
}

fn sqlite_integer(value: u64) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|_| invalid_data("history archive value exceeds SQLite INTEGER"))
}

fn invalid_data(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(IoError::new(
        ErrorKind::InvalidData,
        message.into(),
    )))
}

struct StoredChunk {
    start_byte: u64,
    start_char: u64,
    bytes: u64,
    chars: u64,
    payload: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::run_migrations;
    use tempfile::tempdir;

    fn seed_conversation(connection: &Connection, conversation_id: &str, message_id: &str) {
        connection
            .execute(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES (?1, NULL, NULL, 'archive', 1, 1, NULL, NULL, NULL)",
                [conversation_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES (?1, ?2, 'assistant', '', 'sent', NULL, NULL, 1, 0)",
                params![message_id, conversation_id],
            )
            .unwrap();
    }

    #[test]
    fn multi_megabyte_archive_survives_reopen_and_pages_losslessly() {
        let fixture = tempdir().unwrap();
        let database = fixture.path().join("archive.sqlite");
        let content = format!(
            "{{\"content\":\"{}\",\"tail\":\"完成\"}}",
            "archive-正文-".repeat(300_000)
        );
        let expected_hash = content_hash(content.as_bytes());
        let archive_ref = {
            let mut connection = Connection::open(&database).unwrap();
            run_migrations(&connection).unwrap();
            seed_conversation(&connection, "conversation-1", "assistant-1");
            let descriptor = store_archive(
                &mut connection,
                &ConversationHistoryArchiveInput {
                    conversation_id: "conversation-1".to_string(),
                    assistant_message_id: "assistant-1".to_string(),
                    sequence: 7,
                    call_id: "call-1".to_string(),
                    tool: "web_fetch".to_string(),
                    content_type: "application/json".to_string(),
                    content: content.clone(),
                    truncated_at_source: false,
                    model_projection_truncated: false,
                    archive_projection_truncated: false,
                    created_at: 2,
                },
            )
            .unwrap();
            assert_eq!(descriptor.content_hash, expected_hash);
            assert_eq!(descriptor.total_bytes, content.len() as u64);
            assert!(descriptor.chunk_count > 1);
            descriptor.archive_ref
        };

        let connection = Connection::open(&database).unwrap();
        run_migrations(&connection).unwrap();
        let mut restored = String::new();
        let mut cursor = 0;
        loop {
            let page = read_archive_page(
                &connection,
                "conversation-1",
                &archive_ref,
                ConversationHistoryArchivePageUnit::Char,
                cursor,
                37_777,
            )
            .unwrap()
            .unwrap();
            restored.push_str(&page.content);
            let Some(next) = page.next_cursor else {
                break;
            };
            cursor = next;
        }
        assert_eq!(restored, content);
        assert_eq!(content_hash(restored.as_bytes()), expected_hash);

        assert!(read_archive_page(
            &connection,
            "conversation-other",
            &archive_ref,
            ConversationHistoryArchivePageUnit::Char,
            0,
            10,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn byte_pages_preserve_utf8_boundaries_and_delete_cascades() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        seed_conversation(&connection, "conversation-1", "assistant-1");
        let content = "{\"content\":\"甲乙丙丁abcdef\"}".repeat(40_000);
        let archive = store_archive(
            &mut connection,
            &ConversationHistoryArchiveInput {
                conversation_id: "conversation-1".to_string(),
                assistant_message_id: "assistant-1".to_string(),
                sequence: 1,
                call_id: "call-1".to_string(),
                tool: "read_file".to_string(),
                content_type: "application/json".to_string(),
                content: content.clone(),
                truncated_at_source: true,
                model_projection_truncated: false,
                archive_projection_truncated: false,
                created_at: 2,
            },
        )
        .unwrap();
        let mut restored = String::new();
        let mut cursor = 0;
        loop {
            let page = read_archive_page(
                &connection,
                "conversation-1",
                &archive.archive_ref,
                ConversationHistoryArchivePageUnit::Byte,
                cursor,
                8_193,
            )
            .unwrap()
            .unwrap();
            restored.push_str(&page.content);
            let Some(next) = page.next_cursor else {
                break;
            };
            assert!(next > cursor);
            cursor = next;
        }
        assert_eq!(restored, content);
        assert!(archive.truncated_at_source);
        assert!(
            archive.archived_completely,
            "source truncation must not imply that persistence lost bytes from the received payload"
        );

        connection
            .execute("DELETE FROM conversations WHERE id = 'conversation-1'", [])
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_history_blobs",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_history_blob_chunks",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn archive_match_returns_the_exact_character_offset_case_insensitively() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        seed_conversation(&connection, "conversation-1", "assistant-1");
        let content = "甲乙丙 Prefix Exact-Needle 后文".to_string();
        let archive = store_archive(
            &mut connection,
            &ConversationHistoryArchiveInput {
                conversation_id: "conversation-1".to_string(),
                assistant_message_id: "assistant-1".to_string(),
                sequence: 1,
                call_id: "call-1".to_string(),
                tool: "web_fetch".to_string(),
                content_type: "text/plain".to_string(),
                content: content.clone(),
                truncated_at_source: false,
                model_projection_truncated: true,
                archive_projection_truncated: false,
                created_at: 2,
            },
        )
        .unwrap();

        let offset = find_archive_match_char_offset(
            &connection,
            "conversation-1",
            &archive.archive_ref,
            "exact-needle",
        )
        .unwrap()
        .unwrap();

        let expected = content[..content.find("Exact-Needle").unwrap()]
            .chars()
            .count() as u64;
        assert_eq!(offset, expected);
        assert!(find_archive_match_char_offset(
            &connection,
            "conversation-other",
            &archive.archive_ref,
            "exact-needle"
        )
        .unwrap()
        .is_none());
    }
}
