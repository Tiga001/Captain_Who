//! Freeze expensive archive material before acquiring the database writer. The commit consumes
//! only prepared bytes; it never opens the capture file or runs a compressor.
use super::*;
use std::borrow::Cow;

struct PreparedChunk {
    start_byte: u64,
    start_char: u64,
    bytes: u64,
    chars: u64,
    payload: Vec<u8>,
}

pub(crate) struct PreparedArchive<'a> {
    descriptor: ConversationHistoryArchiveDescriptor,
    content: Cow<'a, str>,
    chunks: Vec<PreparedChunk>,
    chunk_bytes: usize,
    mismatch: &'static str,
}

impl PreparedArchive<'_> {
    pub(crate) fn estimated_bytes(&self) -> usize {
        let d = &self.descriptor;
        // Include caller metadata plus its frozen descriptor, FTS text, compressed bytes and
        // vector entries. This estimates retained payload, not SQLite's own page/FTS buffers.
        let identities = [
            &d.archive_ref,
            &d.conversation_id,
            &d.assistant_message_id,
            &d.call_id,
            &d.tool,
            &d.content_type,
            &d.content_hash,
        ]
        .iter()
        .fold(0_usize, |size, value| size.saturating_add(value.len()));
        self.content
            .len()
            .saturating_add(identities.saturating_mul(2))
            .saturating_add(4096)
            .saturating_add(self.chunk_bytes)
    }
    fn check_budget(&self, maximum: usize) -> rusqlite::Result<()> {
        if self.estimated_bytes() > maximum {
            return Err(invalid_data("storage_executor_job_too_large"));
        }
        Ok(())
    }
    fn append(&mut self, chunk: &str, maximum: usize) -> rusqlite::Result<()> {
        let payload = zstd::stream::encode_all(chunk.as_bytes(), ZSTD_LEVEL)
            .map_err(|error| invalid_data(format!("cannot compress history archive: {error}")))?;
        self.chunk_bytes = self
            .chunk_bytes
            .saturating_add(payload.len())
            .saturating_add(std::mem::size_of::<PreparedChunk>());
        self.chunks.push(PreparedChunk {
            start_byte: self.descriptor.total_bytes,
            start_char: self.descriptor.total_chars,
            bytes: chunk.len() as u64,
            chars: chunk.chars().count() as u64,
            payload,
        });
        self.descriptor.total_bytes += chunk.len() as u64;
        self.descriptor.total_chars += chunk.chars().count() as u64;
        self.descriptor.chunk_count += 1;
        self.check_budget(maximum)
    }
}

pub(crate) fn prepare_archive(
    input: &ConversationHistoryArchiveInput,
    maximum_bytes: usize,
) -> rusqlite::Result<PreparedArchive<'_>> {
    validate_input(input)?;
    let mut prepared = PreparedArchive {
        descriptor: descriptor(input),
        content: Cow::Borrowed(&input.content),
        chunks: Vec::new(),
        chunk_bytes: 0,
        mismatch: "history archive identity was reused for different tool-result content",
    };
    prepared.check_budget(maximum_bytes)?;
    prepared.descriptor.content_hash = content_hash(input.content.as_bytes());
    for chunk in split_utf8_chunks(&input.content) {
        prepared.append(chunk, maximum_bytes)?;
    }
    Ok(prepared)
}

pub(crate) fn prepare_archive_file(
    input: &ConversationHistoryArchiveFileInput,
    maximum_bytes: usize,
) -> rusqlite::Result<PreparedArchive<'static>> {
    prepare_archive_file_before_verification(input, maximum_bytes, || {})
}

fn prepare_archive_file_before_verification(
    input: &ConversationHistoryArchiveFileInput,
    maximum_bytes: usize,
    before_verification: impl FnOnce(),
) -> rusqlite::Result<PreparedArchive<'static>> {
    validate_file_input(input)?;
    let identity = ConversationHistoryArchiveInput {
        conversation_id: input.conversation_id.clone(),
        assistant_message_id: input.assistant_message_id.clone(),
        sequence: input.sequence,
        call_id: input.call_id.clone(),
        tool: input.tool.clone(),
        content_type: input.content_type.clone(),
        content: String::new(),
        truncated_at_source: input.truncated_at_source,
        model_projection_truncated: input.model_projection_truncated,
        archive_projection_truncated: input.archive_projection_truncated,
        created_at: input.created_at,
    };
    let mut prepared = PreparedArchive {
        descriptor: descriptor(&identity),
        content: Cow::Owned(String::new()),
        chunks: Vec::new(),
        chunk_bytes: 0,
        mismatch:
            "history archive identity was reused for different file-backed tool-result content",
    };
    let mut file = File::open(&input.content_path).map_err(|error| {
        invalid_data(format!(
            "cannot open file-backed history archive for compression: {error}"
        ))
    })?;
    let before = file
        .metadata()
        .map_err(|error| invalid_data(error.to_string()))?;
    if before.len() > maximum_bytes as u64 {
        return Err(invalid_data("storage_executor_job_too_large"));
    }
    let mut digest = Sha256::new();
    for_each_utf8_chunk(&mut file, |chunk| {
        // Retain exactly the same frozen text that is compressed and hashed for the FTS row.
        // A growing capture is rejected before retaining an unbounded contiguous allocation.
        if chunk.len() > maximum_bytes.saturating_sub(prepared.estimated_bytes()) {
            return Err(invalid_data("storage_executor_job_too_large"));
        }
        prepared.content.to_mut().push_str(chunk);
        digest.update(chunk.as_bytes());
        prepared.append(chunk, maximum_bytes)
    })?;
    prepared.descriptor.content_hash = format!("{CONTENT_HASH_PREFIX}{:x}", digest.finalize());
    let after = file
        .metadata()
        .map_err(|error| invalid_data(error.to_string()))?;
    // Re-scan outside the connection as well: same-size rewrites must not silently mismatch the
    // archive hash, compressed chunks and searchable text. Once frozen, later source removal or
    // mutation cannot change the prepared job, which owns all bytes required by its transaction.
    before_verification();
    let verified = scan_utf8_file(&input.content_path, prepared.descriptor.total_bytes)?;
    if before.len() != after.len()
        || before.len() != prepared.descriptor.total_bytes
        || before.modified().ok() != after.modified().ok()
        || verified.content_hash != prepared.descriptor.content_hash
        || verified.total_bytes != prepared.descriptor.total_bytes
        || verified.total_chars != prepared.descriptor.total_chars
        || verified.chunk_count != prepared.descriptor.chunk_count
    {
        return Err(invalid_data(
            "file-backed history archive changed while it was being stored",
        ));
    }
    prepared.check_budget(maximum_bytes)?;
    Ok(prepared)
}

fn descriptor(input: &ConversationHistoryArchiveInput) -> ConversationHistoryArchiveDescriptor {
    ConversationHistoryArchiveDescriptor {
        archive_ref: format!("{ARCHIVE_REF_PREFIX}{}", Uuid::new_v4()),
        conversation_id: input.conversation_id.clone(),
        assistant_message_id: input.assistant_message_id.clone(),
        sequence: input.sequence,
        call_id: input.call_id.clone(),
        tool: input.tool.clone(),
        content_type: input.content_type.clone(),
        content_hash: String::new(),
        total_bytes: 0,
        total_chars: 0,
        chunk_count: 0,
        compression: "zstd".into(),
        truncated_at_source: input.truncated_at_source,
        archived_completely: true,
        model_projection_truncated: input.model_projection_truncated,
        archive_projection_truncated: input.archive_projection_truncated,
        created_at: input.created_at,
    }
}

pub(crate) fn store_prepared_archive(
    connection: &mut Connection,
    prepared: &PreparedArchive<'_>,
) -> rusqlite::Result<ConversationHistoryArchiveDescriptor> {
    let d = &prepared.descriptor;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(existing) = find_archive_for_trace_item(
        &transaction,
        &d.conversation_id,
        &d.assistant_message_id,
        d.sequence,
    )? {
        if existing.call_id != d.call_id
            || existing.tool != d.tool
            || existing.content_type != d.content_type
            || existing.content_hash != d.content_hash
            || existing.total_bytes != d.total_bytes
            || existing.total_chars != d.total_chars
            || existing.truncated_at_source != d.truncated_at_source
            || existing.model_projection_truncated != d.model_projection_truncated
            || existing.archive_projection_truncated != d.archive_projection_truncated
        {
            return Err(invalid_data(prepared.mismatch));
        }
        index_archive_content(&transaction, &existing, &prepared.content)?;
        transaction.commit()?;
        return Ok(existing);
    }
    transaction.execute(
        "INSERT INTO conversation_history_blobs (
            archive_ref,conversation_id,assistant_message_id,sequence,call_id,tool,content_type,
            content_hash,uncompressed_bytes,uncompressed_chars,chunk_count,compression,
            truncated_at_source,archived_completely,model_projection_truncated,
            archive_projection_truncated,created_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'zstd',?12,1,?13,?14,?15)",
        params![
            &d.archive_ref,
            &d.conversation_id,
            &d.assistant_message_id,
            sqlite_integer(d.sequence)?,
            &d.call_id,
            &d.tool,
            &d.content_type,
            &d.content_hash,
            sqlite_integer(d.total_bytes)?,
            sqlite_integer(d.total_chars)?,
            sqlite_integer(d.chunk_count)?,
            d.truncated_at_source,
            d.model_projection_truncated,
            d.archive_projection_truncated,
            d.created_at
        ],
    )?;
    for (index, chunk) in prepared.chunks.iter().enumerate() {
        transaction.execute(
            "INSERT INTO conversation_history_blob_chunks (
                archive_ref,chunk_index,uncompressed_start_byte,uncompressed_start_char,
                uncompressed_bytes,uncompressed_chars,compressed_bytes,payload
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                &d.archive_ref,
                sqlite_integer(index as u64)?,
                sqlite_integer(chunk.start_byte)?,
                sqlite_integer(chunk.start_char)?,
                sqlite_integer(chunk.bytes)?,
                sqlite_integer(chunk.chars)?,
                sqlite_integer(chunk.payload.len() as u64)?,
                &chunk.payload
            ],
        )?;
    }
    index_archive_content(&transaction, d, &prepared.content)?;
    transaction.commit()?;
    find_archive_by_ref(connection, &d.conversation_id, &d.archive_ref)?
        .ok_or_else(|| invalid_data("stored history archive cannot be reloaded"))
}

#[cfg(test)]
mod tests;
