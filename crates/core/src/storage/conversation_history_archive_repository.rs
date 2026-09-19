//! Lossless, conversation-scoped storage for sanitized tool-result observations.
//!
//! Durable conversation traces intentionally retain bounded projections. This archive stores the
//! security-sanitized textual result before those length limits are applied, compressed in
//! independently readable UTF-8 chunks.

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Error as IoError, ErrorKind, Read};
use std::path::PathBuf;
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

/// File-backed variant used by streaming tool captures.
///
/// `content_path` must contain the complete, security-sanitized UTF-8 archive projection. The
/// repository reads and compresses it incrementally; callers retain ownership of the temporary
/// file and may remove it after this call returns.
#[derive(Debug, Clone)]
pub struct ConversationHistoryArchiveFileInput {
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub sequence: u64,
    pub call_id: String,
    pub tool: String,
    pub content_type: String,
    pub content_path: PathBuf,
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
        let transaction = connection.transaction()?;
        index_archive_content(&transaction, &existing, &input.content)?;
        transaction.commit()?;
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

/// Removes an exact-history blob which was prepared for a terminal ToolResult but never became
/// reachable from the authoritative Trace boundary.
///
/// This is intentionally an exact compare-and-delete operation, not general archive cleanup. It
/// is used only after the pending-action settlement inspector has proved that the attempted
/// terminal audit/Trace transaction is definitely absent. A different blob, a referenced blob,
/// or a command-owned blob always fails closed.
pub fn delete_unreferenced_exact_archive(
    connection: &mut Connection,
    input: &ConversationHistoryArchiveInput,
) -> rusqlite::Result<bool> {
    validate_input(input)?;
    let Some(existing) = find_archive_for_trace_item(
        connection,
        &input.conversation_id,
        &input.assistant_message_id,
        input.sequence,
    )?
    else {
        return Ok(false);
    };
    let content_bytes = input.content.as_bytes();
    let expected_content_hash = content_hash(content_bytes);
    if existing.call_id != input.call_id
        || existing.tool != input.tool
        || existing.content_type != input.content_type
        || existing.content_hash != expected_content_hash
        || existing.total_bytes != content_bytes.len() as u64
        || existing.total_chars != input.content.chars().count() as u64
        || existing.truncated_at_source != input.truncated_at_source
        || existing.model_projection_truncated != input.model_projection_truncated
        || existing.archive_projection_truncated != input.archive_projection_truncated
    {
        return Err(invalid_data(
            "uncommitted history archive does not match the exact attempted ToolResult",
        ));
    }

    let referenced: bool = connection.query_row(
        "SELECT EXISTS (
             SELECT 1
             FROM conversation_turn_trace_items
             WHERE assistant_message_id = ?1
               AND sequence = ?2
               AND json_extract(item_json, '$.archiveRef') = ?3
             UNION ALL
             SELECT 1 FROM agent_command_sessions WHERE archive_ref = ?3
             UNION ALL
             SELECT 1 FROM agent_command_session_lifecycle_events WHERE archive_ref = ?3
         )",
        params![
            &input.assistant_message_id,
            sqlite_integer(input.sequence)?,
            &existing.archive_ref,
        ],
        |row| row.get(0),
    )?;
    if referenced {
        return Err(invalid_data(
            "uncommitted history archive is already referenced by durable state",
        ));
    }

    let transaction = connection.transaction()?;
    let deleted = transaction.execute(
        "DELETE FROM conversation_history_blobs
         WHERE archive_ref = ?1
           AND conversation_id = ?2
           AND assistant_message_id = ?3
           AND sequence = ?4
           AND call_id = ?5
           AND tool = ?6
           AND content_type = ?7
           AND content_hash = ?8
           AND uncompressed_bytes = ?9
           AND uncompressed_chars = ?10
           AND truncated_at_source = ?11
           AND model_projection_truncated = ?12
           AND archive_projection_truncated = ?13
           AND NOT EXISTS (
               SELECT 1
               FROM conversation_turn_trace_items
               WHERE assistant_message_id = ?3
                 AND sequence = ?4
                 AND json_extract(item_json, '$.archiveRef') = ?1
           )
           AND NOT EXISTS (SELECT 1 FROM agent_command_sessions WHERE archive_ref = ?1)
           AND NOT EXISTS (
               SELECT 1 FROM agent_command_session_lifecycle_events WHERE archive_ref = ?1
           )",
        params![
            &existing.archive_ref,
            &input.conversation_id,
            &input.assistant_message_id,
            sqlite_integer(input.sequence)?,
            &input.call_id,
            &input.tool,
            &input.content_type,
            &expected_content_hash,
            sqlite_integer(content_bytes.len() as u64)?,
            sqlite_integer(input.content.chars().count() as u64)?,
            input.truncated_at_source,
            input.model_projection_truncated,
            input.archive_projection_truncated,
        ],
    )?;
    if deleted > 1 {
        return Err(invalid_data(
            "uncommitted history archive deletion affected multiple rows",
        ));
    }
    transaction.commit()?;
    Ok(deleted == 1)
}

/// Stores one already-materialized UTF-8 Tool Result without loading its complete body into the
/// archive compressor.
///
/// FTS5 still requires one transient contiguous string when the search row is inserted. That
/// allocation happens only after the immutable zstd chunks have committed and is bounded by the
/// shared capture safety quota at the process-capture layer.
pub fn store_archive_file(
    connection: &mut Connection,
    input: &ConversationHistoryArchiveFileInput,
) -> rusqlite::Result<ConversationHistoryArchiveDescriptor> {
    validate_file_input(input)?;
    let stats = scan_utf8_file(&input.content_path)?;
    if let Some(existing) = find_archive_for_trace_item(
        connection,
        &input.conversation_id,
        &input.assistant_message_id,
        input.sequence,
    )? {
        if existing.call_id != input.call_id
            || existing.tool != input.tool
            || existing.content_type != input.content_type
            || existing.content_hash != stats.content_hash
            || existing.total_bytes != stats.total_bytes
            || existing.total_chars != stats.total_chars
            || existing.truncated_at_source != input.truncated_at_source
            || existing.model_projection_truncated != input.model_projection_truncated
            || existing.archive_projection_truncated != input.archive_projection_truncated
        {
            return Err(invalid_data(
                "history archive identity was reused for different file-backed tool-result content",
            ));
        }
        let content = std::fs::read_to_string(&input.content_path).map_err(|error| {
            invalid_data(format!(
                "cannot read file-backed history archive for FTS indexing: {error}"
            ))
        })?;
        let transaction = connection.transaction()?;
        index_archive_content(&transaction, &existing, &content)?;
        transaction.commit()?;
        return Ok(existing);
    }

    let archive_ref = format!("{ARCHIVE_REF_PREFIX}{}", Uuid::new_v4());
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
            &stats.content_hash,
            sqlite_integer(stats.total_bytes)?,
            sqlite_integer(stats.total_chars)?,
            sqlite_integer(stats.chunk_count)?,
            input.truncated_at_source,
            input.model_projection_truncated,
            input.archive_projection_truncated,
            input.created_at,
        ],
    )?;

    let mut file = File::open(&input.content_path).map_err(|error| {
        invalid_data(format!(
            "cannot open file-backed history archive for compression: {error}"
        ))
    })?;
    let mut chunk_index = 0_u64;
    let mut byte_offset = 0_u64;
    let mut char_offset = 0_u64;
    for_each_utf8_chunk(&mut file, |chunk| {
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
                sqlite_integer(chunk_index)?,
                sqlite_integer(byte_offset)?,
                sqlite_integer(char_offset)?,
                sqlite_integer(chunk_bytes)?,
                sqlite_integer(chunk_chars)?,
                sqlite_integer(compressed.len() as u64)?,
                compressed,
            ],
        )?;
        chunk_index = chunk_index.saturating_add(1);
        byte_offset = byte_offset.saturating_add(chunk_bytes);
        char_offset = char_offset.saturating_add(chunk_chars);
        Ok(())
    })?;
    if chunk_index != stats.chunk_count
        || byte_offset != stats.total_bytes
        || char_offset != stats.total_chars
    {
        return Err(invalid_data(
            "file-backed history archive changed while it was being stored",
        ));
    }

    let descriptor = ConversationHistoryArchiveDescriptor {
        archive_ref: archive_ref.clone(),
        conversation_id: input.conversation_id.clone(),
        assistant_message_id: input.assistant_message_id.clone(),
        sequence: input.sequence,
        call_id: input.call_id.clone(),
        tool: input.tool.clone(),
        content_type: input.content_type.clone(),
        content_hash: stats.content_hash,
        total_bytes: stats.total_bytes,
        total_chars: stats.total_chars,
        chunk_count: stats.chunk_count,
        compression: "zstd".to_string(),
        truncated_at_source: input.truncated_at_source,
        archived_completely: true,
        model_projection_truncated: input.model_projection_truncated,
        archive_projection_truncated: input.archive_projection_truncated,
        created_at: input.created_at,
    };
    let content = std::fs::read_to_string(&input.content_path).map_err(|error| {
        invalid_data(format!(
            "cannot read file-backed history archive for FTS indexing: {error}"
        ))
    })?;
    index_archive_content(&transaction, &descriptor, &content)?;
    transaction.commit()?;

    find_archive_by_ref(connection, &input.conversation_id, &archive_ref)?
        .ok_or_else(|| invalid_data("stored file-backed history archive cannot be reloaded"))
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
) -> rusqlite::Result<ConversationHistoryArchiveForkCopy> {
    let source = find_archive_by_ref(connection, source_conversation_id, source_archive_ref)?
        .ok_or_else(|| invalid_data("fork trace references a missing history archive"))?;
    Ok(ConversationHistoryArchiveForkCopy {
        source_archive_ref: source_archive_ref.to_string(),
        target_archive_ref: format!("{ARCHIVE_REF_PREFIX}{}", Uuid::new_v4()),
        target_conversation_id: target_conversation_id.to_string(),
        target_assistant_message_id: target_assistant_message_id.to_string(),
        // The Archive owns its durable identity. A command Session lifecycle item deliberately
        // points at the same logical tool call while its exact terminal output uses the distinct
        // `command-session:<sessionId>` Archive call id. Re-deriving this field from the Trace item
        // collapses those two archives and violates the per-conversation uniqueness constraint.
        target_call_id: source.call_id,
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
        "INSERT INTO conversation_history_index_entries (ref_key, archive_ref, owner_message_id)
         VALUES ('archive:' || ?1, ?1, ?2)",
        params![&copy.target_archive_ref, &copy.target_assistant_message_id],
    )?;
    let indexed_rows = connection.execute(
        "
        INSERT INTO conversation_history_fts (
            rowid,
            ref_key, conversation_id, record_type, item_kind, message_id,
            assistant_message_id, sequence, archive_ref, call_id, tool,
            status, run_id, created_at, position, within_message_order, content
        )
        SELECT
            (SELECT rowid FROM conversation_history_index_entries WHERE ref_key = 'archive:' || ?1),
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
        INNER JOIN conversation_history_index_entries AS identity
            ON identity.ref_key = 'archive:' || source.archive_ref
        INNER JOIN conversation_history_fts AS indexed
            ON indexed.rowid = identity.rowid
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
    if indexed_rows != 1 {
        return Err(invalid_data(
            "fork archive is missing its searchable projection",
        ));
    }
    Ok(())
}

fn index_archive_content(
    connection: &Connection,
    descriptor: &ConversationHistoryArchiveDescriptor,
    content: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM conversation_history_index_entries WHERE ref_key = ?1",
        [format!("archive:{}", descriptor.archive_ref)],
    )?;
    connection.execute(
        "INSERT INTO conversation_history_index_entries (ref_key, archive_ref, owner_message_id)
         VALUES ('archive:' || ?1, ?1, ?2)",
        params![&descriptor.archive_ref, &descriptor.assistant_message_id],
    )?;
    connection.execute(
        "
        INSERT INTO conversation_history_fts (
            rowid,
            ref_key, conversation_id, record_type, item_kind, message_id,
            assistant_message_id, sequence, archive_ref, call_id, tool,
            status, run_id, created_at, position, within_message_order, content
        ) VALUES (
            (SELECT rowid FROM conversation_history_index_entries WHERE ref_key = ?1),
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

fn validate_file_input(input: &ConversationHistoryArchiveFileInput) -> rusqlite::Result<()> {
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
    if !input.content_path.is_file() {
        return Err(invalid_data(
            "file-backed history archive content does not exist",
        ));
    }
    Ok(())
}

struct ArchiveFileStats {
    content_hash: String,
    total_bytes: u64,
    total_chars: u64,
    chunk_count: u64,
}

fn scan_utf8_file(path: &std::path::Path) -> rusqlite::Result<ArchiveFileStats> {
    let mut file = File::open(path).map_err(|error| {
        invalid_data(format!(
            "cannot open file-backed history archive for validation: {error}"
        ))
    })?;
    let mut digest = Sha256::new();
    let mut total_bytes = 0_u64;
    let mut total_chars = 0_u64;
    let mut chunk_count = 0_u64;
    for_each_utf8_chunk(&mut file, |chunk| {
        digest.update(chunk.as_bytes());
        total_bytes = total_bytes.saturating_add(chunk.len() as u64);
        total_chars = total_chars.saturating_add(chunk.chars().count() as u64);
        chunk_count = chunk_count.saturating_add(1);
        Ok(())
    })?;
    Ok(ArchiveFileStats {
        content_hash: format!("{CONTENT_HASH_PREFIX}{:x}", digest.finalize()),
        total_bytes,
        total_chars,
        chunk_count,
    })
}

fn for_each_utf8_chunk(
    reader: &mut impl Read,
    mut consume: impl FnMut(&str) -> rusqlite::Result<()>,
) -> rusqlite::Result<()> {
    const READ_BYTES: usize = 64 * 1024;
    let mut read_buffer = [0_u8; READ_BYTES];
    let mut pending = Vec::<u8>::with_capacity(CHUNK_TARGET_BYTES + READ_BYTES);
    let mut emitted = false;
    loop {
        let read = reader.read(&mut read_buffer).map_err(|error| {
            invalid_data(format!("cannot read file-backed history archive: {error}"))
        })?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&read_buffer[..read]);
        while pending.len() >= CHUNK_TARGET_BYTES {
            let mut end = CHUNK_TARGET_BYTES;
            while end > 0 && std::str::from_utf8(&pending[..end]).is_err() {
                end -= 1;
                if CHUNK_TARGET_BYTES.saturating_sub(end) > 4 {
                    return Err(invalid_data(
                        "file-backed history archive contains invalid UTF-8",
                    ));
                }
            }
            if end == 0 {
                return Err(invalid_data(
                    "file-backed history archive contains invalid UTF-8",
                ));
            }
            let chunk = std::str::from_utf8(&pending[..end]).map_err(|error| {
                invalid_data(format!(
                    "file-backed history archive contains invalid UTF-8: {error}"
                ))
            })?;
            consume(chunk)?;
            emitted = true;
            pending.drain(..end);
        }
    }
    if !pending.is_empty() {
        let chunk = std::str::from_utf8(&pending).map_err(|error| {
            invalid_data(format!(
                "file-backed history archive contains invalid UTF-8: {error}"
            ))
        })?;
        consume(chunk)?;
        emitted = true;
    }
    if !emitted {
        consume("")?;
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
    use std::io::Write;
    use tempfile::{tempdir, NamedTempFile};

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
                    created_at, position
                 ) VALUES (?1, ?2, 'assistant', '', 'sent', NULL, 1, 0)",
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
    fn file_backed_archive_streams_chunks_and_restores_the_exact_hash() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        seed_conversation(&connection, "conversation-file", "assistant-file");
        let content = format!(
            "{{\"stdout\":\"{}\",\"stderr\":\"尾部\"}}",
            "streamed-正文-".repeat(180_000)
        );
        let expected_hash = content_hash(content.as_bytes());
        let mut source = NamedTempFile::new().unwrap();
        source.write_all(content.as_bytes()).unwrap();
        source.flush().unwrap();

        let descriptor = store_archive_file(
            &mut connection,
            &ConversationHistoryArchiveFileInput {
                conversation_id: "conversation-file".to_string(),
                assistant_message_id: "assistant-file".to_string(),
                sequence: 3,
                call_id: "call-file".to_string(),
                tool: "run_command".to_string(),
                content_type: "application/vnd.mycopilot.agent-tool-result+json".to_string(),
                content_path: source.path().to_path_buf(),
                truncated_at_source: false,
                model_projection_truncated: true,
                archive_projection_truncated: false,
                created_at: 2,
            },
        )
        .unwrap();

        assert_eq!(descriptor.content_hash, expected_hash);
        assert_eq!(descriptor.total_bytes, content.len() as u64);
        assert!(descriptor.chunk_count > 1);
        let restored = read_complete_archive_content(&connection, &descriptor).unwrap();
        assert_eq!(restored, content);
        assert_eq!(content_hash(restored.as_bytes()), expected_hash);
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

    #[test]
    fn archive_search_identity_survives_reindex_fork_status_update_and_delete() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        seed_conversation(&connection, "source", "source-assistant");
        seed_conversation(&connection, "fork", "fork-assistant");
        let input = ConversationHistoryArchiveInput {
            conversation_id: "source".to_string(),
            assistant_message_id: "source-assistant".to_string(),
            sequence: 0,
            call_id: "call-source".to_string(),
            tool: "read_file".to_string(),
            content_type: "text/plain".to_string(),
            content: "Full preserved searchable archive 独立分支".repeat(100),
            truncated_at_source: false,
            model_projection_truncated: true,
            archive_projection_truncated: false,
            created_at: 2,
        };
        let source = store_archive(&mut connection, &input).unwrap();
        assert_eq!(store_archive(&mut connection, &input).unwrap(), source);
        let copy = load_fork_copy(
            &connection,
            "source",
            &source.archive_ref,
            "fork",
            "fork-assistant",
        )
        .unwrap();
        {
            let transaction = connection.transaction().unwrap();
            clone_archive_in_connection(&transaction, &copy).unwrap();
            transaction.commit().unwrap();
        }
        let identities: i64 = connection
            .query_row(
                "SELECT count(*) FROM conversation_history_index_entries WHERE ref_key=?1",
                [format!("archive:{}", source.archive_ref)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(identities, 1);
        assert_eq!(connection.query_row("SELECT content FROM conversation_history_fts AS f JOIN conversation_history_index_entries AS i ON i.rowid=f.rowid WHERE i.ref_key=?1", [format!("archive:{}",copy.target_archive_ref)], |row|row.get::<_,String>(0)).unwrap(),input.content);
        connection.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at,completed_at) VALUES ('fork-assistant','fork','fork-run',6,'completed',0,1,2,2)", []).unwrap();
        connection.execute("INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json) VALUES ('fork-assistant',0,'backend_state',?1)", [serde_json::json!({"type":"backend_state","sequence":0,"archiveRef":copy.target_archive_ref,"status":"completed"}).to_string()]).unwrap();
        let updated:i64=connection.query_row("SELECT count(*) FROM conversation_history_fts AS f JOIN conversation_history_index_entries AS i ON i.rowid=f.rowid WHERE i.archive_ref=?1 AND f.status='completed' AND f.run_id='fork-run'",[&copy.target_archive_ref],|row|row.get(0)).unwrap();
        assert_eq!(
            updated, 2,
            "both archive and referencing Trace inherit final status/run"
        );
        connection
            .execute("DELETE FROM conversations WHERE id='source'", [])
            .unwrap();
        let fork_archive = find_archive_by_ref(&connection, "fork", &copy.target_archive_ref)
            .unwrap()
            .unwrap();
        assert_eq!(
            read_complete_archive_content(&connection, &fork_archive).unwrap(),
            input.content
        );
        connection
            .execute(
                "DELETE FROM conversation_history_blobs WHERE archive_ref=?1",
                [&copy.target_archive_ref],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM conversation_history_index_entries WHERE ref_key=?1",
                    [format!("archive:{}", copy.target_archive_ref)],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        connection
            .execute("DELETE FROM conversations WHERE id='fork'", [])
            .unwrap();
        for table in [
            "conversation_history_fts",
            "conversation_history_index_entries",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn idempotent_archive_reindex_failure_restores_identity_and_full_text_atomically() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        seed_conversation(&connection, "source", "source-assistant");
        let input = ConversationHistoryArchiveInput {
            conversation_id: "source".to_string(),
            assistant_message_id: "source-assistant".to_string(),
            sequence: 0,
            call_id: "call".to_string(),
            tool: "read_file".to_string(),
            content_type: "text/plain".to_string(),
            content: "Original exact searchable archive".to_string(),
            truncated_at_source: false,
            model_projection_truncated: false,
            archive_projection_truncated: false,
            created_at: 2,
        };
        let archive = store_archive(&mut connection, &input).unwrap();
        fn indexed(connection: &Connection, archive_ref: &str) -> (i64, String) {
            connection.query_row("SELECT i.rowid,f.content FROM conversation_history_index_entries AS i JOIN conversation_history_fts AS f ON f.rowid=i.rowid WHERE i.ref_key=?1",[format!("archive:{archive_ref}")],|row|Ok((row.get(0)?,row.get(1)?))).unwrap()
        }
        let before = indexed(&connection, &archive.archive_ref);
        connection.execute_batch("CREATE TRIGGER fail_archive_index BEFORE INSERT ON conversation_history_index_entries WHEN NEW.ref_key LIKE 'archive:%' BEGIN SELECT RAISE(ABORT,'injected index failure'); END;").unwrap();
        assert!(store_archive(&mut connection, &input)
            .unwrap_err()
            .to_string()
            .contains("injected index failure"));
        assert_eq!(indexed(&connection, &archive.archive_ref), before);
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(input.content.as_bytes()).unwrap();
        let file_input = ConversationHistoryArchiveFileInput {
            conversation_id: input.conversation_id,
            assistant_message_id: input.assistant_message_id,
            sequence: input.sequence,
            call_id: input.call_id,
            tool: input.tool,
            content_type: input.content_type,
            content_path: file.path().to_path_buf(),
            truncated_at_source: false,
            model_projection_truncated: false,
            archive_projection_truncated: false,
            created_at: input.created_at,
        };
        assert!(store_archive_file(&mut connection, &file_input)
            .unwrap_err()
            .to_string()
            .contains("injected index failure"));
        assert_eq!(indexed(&connection, &archive.archive_ref), before);
    }
}
