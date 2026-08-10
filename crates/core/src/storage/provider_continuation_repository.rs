use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use uuid::Version;

pub(crate) const PROVIDER_CONTINUATION_RECORD_SCHEMA_VERSION: u32 = 1;
pub(crate) const PROVIDER_CONTINUATION_ENVELOPE_VERSION: u32 = 1;
pub(crate) const PROVIDER_CONTINUATION_COMPRESSION: &str = "zstd_binary_v1";
pub(crate) const PROVIDER_CONTINUATION_ENCRYPTION: &str = "chacha20_poly1305_v1";
pub(crate) const PROVIDER_CONTINUATION_REF_PREFIX: &str = "provider-continuation-v1:";

const MAX_IDENTITY_BYTES: usize = 2_048;
const MAX_PROVIDER_CONTINUATION_CIPHERTEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_PROVIDER_CONTINUATION_DECODED_BYTES: usize = 8 * 1024 * 1024;
const NONCE_BYTES: usize = 12;
const SHA256_DIGEST_BYTES: usize = "sha256:".len() + 64;
const MAX_RUNTIME_TOOL_CALLS: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderContinuationRecordState {
    Active,
    Superseded,
    Released,
}

impl ProviderContinuationRecordState {
    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "superseded" => Ok(Self::Superseded),
            "released" => Ok(Self::Released),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProviderContinuationRuntimeToolIdentity {
    pub(crate) provider_tool_index: usize,
    pub(crate) runtime_call_id: String,
}

impl std::fmt::Debug for ProviderContinuationRuntimeToolIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationRuntimeToolIdentity([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProviderContinuationEnvelopeRecord {
    pub(crate) continuation_id: String,
    pub(crate) conversation_id: String,
    pub(crate) assistant_message_id: String,
    pub(crate) run_id: String,
    pub(crate) request_index: u64,
    pub(crate) assistant_turn_id: String,
    pub(crate) assistant_turn_digest: String,
    pub(crate) provider_protocol_digest: String,
    pub(crate) payload_digest: String,
    pub(crate) nonce: Vec<u8>,
    pub(crate) ciphertext: Vec<u8>,
    pub(crate) decoded_bytes: u64,
    pub(crate) compressed_bytes: u64,
    pub(crate) created_at: i64,
    pub(crate) runtime_tool_calls: Vec<ProviderContinuationRuntimeToolIdentity>,
}

impl std::fmt::Debug for ProviderContinuationEnvelopeRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationEnvelopeRecord([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoredProviderContinuationRecord {
    pub(crate) continuation_id: String,
    pub(crate) conversation_id: String,
    pub(crate) assistant_message_id: String,
    pub(crate) run_id: String,
    pub(crate) request_index: u64,
    pub(crate) assistant_turn_id: String,
    pub(crate) assistant_turn_digest: String,
    pub(crate) provider_protocol_digest: String,
    pub(crate) state: ProviderContinuationRecordState,
    pub(crate) superseded_by: Option<String>,
    pub(crate) payload_digest: Option<String>,
    pub(crate) nonce: Option<Vec<u8>>,
    pub(crate) ciphertext: Option<Vec<u8>>,
    pub(crate) decoded_bytes: Option<u64>,
    pub(crate) compressed_bytes: Option<u64>,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
    pub(crate) released_at: Option<i64>,
    /// `None` means ciphertext is sealed but not yet visible to replay/list/fork/approval.
    pub(crate) activated_at: Option<i64>,
}

impl std::fmt::Debug for StoredProviderContinuationRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StoredProviderContinuationRecord([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderContinuationStoreOutcome {
    Inserted { superseded: usize },
    Idempotent { continuation_id: String },
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Exact-ref lifecycle result consumed by the controlled Vault release API.
pub(crate) enum ProviderContinuationReleaseOutcome {
    Released,
    AlreadyReleased,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderContinuationPromotionOutcome {
    Promoted,
    AlreadyPromoted,
    Released,
    NotFound,
}

pub(crate) fn valid_continuation_id(value: &str) -> bool {
    let Some(uuid) = value.strip_prefix(PROVIDER_CONTINUATION_REF_PREFIX) else {
        return false;
    };
    let Ok(uuid) = uuid::Uuid::parse_str(uuid) else {
        return false;
    };
    uuid.get_version() == Some(Version::Random)
        && format!("{PROVIDER_CONTINUATION_REF_PREFIX}{uuid}") == value
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTITY_BYTES
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_sha256_digest(value: &str) -> bool {
    value.len() == SHA256_DIGEST_BYTES
        && value.starts_with("sha256:")
        && value["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_envelope(record: &ProviderContinuationEnvelopeRecord) -> rusqlite::Result<()> {
    let mut previous_provider_index = None;
    let mut runtime_call_ids = std::collections::HashSet::new();
    let runtime_tool_calls_valid = record.runtime_tool_calls.len() <= MAX_RUNTIME_TOOL_CALLS
        && record.runtime_tool_calls.iter().all(|identity| {
            let ordered = previous_provider_index
                .replace(identity.provider_tool_index)
                .is_none_or(|previous| previous < identity.provider_tool_index);
            ordered
                && valid_identity(&identity.runtime_call_id)
                && runtime_call_ids.insert(identity.runtime_call_id.as_str())
        });
    if !valid_continuation_id(&record.continuation_id)
        || !valid_identity(&record.conversation_id)
        || !valid_identity(&record.assistant_message_id)
        || !valid_identity(&record.run_id)
        || !record.assistant_turn_id.starts_with("at1_")
        || record.assistant_turn_id.len() != "at1_".len() + 64
        || !record.assistant_turn_id["at1_".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !valid_sha256_digest(&record.assistant_turn_digest)
        || !valid_sha256_digest(&record.provider_protocol_digest)
        || !valid_sha256_digest(&record.payload_digest)
        || record.nonce.len() != NONCE_BYTES
        || record.ciphertext.len() <= 16
        || record.ciphertext.len() > MAX_PROVIDER_CONTINUATION_CIPHERTEXT_BYTES
        || record.decoded_bytes == 0
        || record.decoded_bytes > MAX_PROVIDER_CONTINUATION_DECODED_BYTES as u64
        || record.compressed_bytes == 0
        || record.compressed_bytes > MAX_PROVIDER_CONTINUATION_CIPHERTEXT_BYTES as u64
        || record.compressed_bytes.saturating_add(16) != record.ciphertext.len() as u64
        || record.created_at < 0
        || !runtime_tool_calls_valid
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation envelope metadata".to_string(),
        ));
    }
    Ok(())
}

fn validate_message_owner(
    connection: &Connection,
    record: &ProviderContinuationEnvelopeRecord,
) -> rusqlite::Result<()> {
    let owner = connection
        .query_row(
            "SELECT conversation_id, role FROM messages WHERE id = ?1",
            [&record.assistant_message_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if owner.as_ref() != Some(&(record.conversation_id.clone(), "assistant".to_string())) {
        return Err(rusqlite::Error::InvalidParameterName(
            "provider continuation owner is not an assistant message in the conversation"
                .to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn store_staged(
    connection: &mut Connection,
    record: &ProviderContinuationEnvelopeRecord,
) -> rusqlite::Result<ProviderContinuationStoreOutcome> {
    validate_envelope(record)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let outcome = store_in_connection(&transaction, record, None)?;
    transaction.commit()?;
    Ok(outcome)
}

/// Inserts an encrypted replay envelope inside a caller-owned transaction.
///
/// Fork uses this entry point only after every source envelope has been decrypted and freshly
/// sealed for its target owner. Keeping the insert in the conversation transaction prevents a
/// fork from becoming visible without all of its provider-native replay state.
pub(crate) fn store_active_in_connection(
    connection: &Connection,
    record: &ProviderContinuationEnvelopeRecord,
) -> rusqlite::Result<ProviderContinuationStoreOutcome> {
    store_in_connection(connection, record, Some(record.created_at))
}

fn store_in_connection(
    connection: &Connection,
    record: &ProviderContinuationEnvelopeRecord,
    activated_at: Option<i64>,
) -> rusqlite::Result<ProviderContinuationStoreOutcome> {
    validate_envelope(record)?;
    validate_message_owner(connection, record)?;

    let existing = load_for_turn(
        connection,
        &record.conversation_id,
        &record.assistant_message_id,
        &record.run_id,
        record.request_index,
    )?;
    if let Some(existing) = existing {
        let existing_runtime_tool_calls =
            load_runtime_tool_identities(connection, &existing.continuation_id)?;
        if existing.state != ProviderContinuationRecordState::Released
            && existing.assistant_turn_id == record.assistant_turn_id
            && existing.assistant_turn_digest == record.assistant_turn_digest
            && existing.provider_protocol_digest == record.provider_protocol_digest
            && existing.payload_digest.as_deref() == Some(record.payload_digest.as_str())
            && existing.decoded_bytes == Some(record.decoded_bytes)
            && existing.compressed_bytes == Some(record.compressed_bytes)
            && existing_runtime_tool_calls == record.runtime_tool_calls
        {
            return Ok(ProviderContinuationStoreOutcome::Idempotent {
                continuation_id: existing.continuation_id,
            });
        }
        return Ok(ProviderContinuationStoreOutcome::Conflict);
    }

    let inserted = connection.execute(
        "
        INSERT INTO provider_continuations (
            continuation_id, schema_version, envelope_version,
            conversation_id, assistant_message_id, run_id, request_index,
            assistant_turn_id, assistant_turn_digest, provider_protocol_digest,
            state, superseded_by, compression, encryption,
            payload_digest, nonce, ciphertext, decoded_bytes, compressed_bytes,
            created_at, updated_at, released_at, activated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
            'active', NULL, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18, NULL, ?19
        )
        ",
        params![
            &record.continuation_id,
            PROVIDER_CONTINUATION_RECORD_SCHEMA_VERSION,
            PROVIDER_CONTINUATION_ENVELOPE_VERSION,
            &record.conversation_id,
            &record.assistant_message_id,
            &record.run_id,
            record.request_index,
            &record.assistant_turn_id,
            &record.assistant_turn_digest,
            &record.provider_protocol_digest,
            PROVIDER_CONTINUATION_COMPRESSION,
            PROVIDER_CONTINUATION_ENCRYPTION,
            &record.payload_digest,
            &record.nonce,
            &record.ciphertext,
            record.decoded_bytes,
            record.compressed_bytes,
            record.created_at,
            activated_at,
        ],
    )?;
    if inserted != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    for identity in &record.runtime_tool_calls {
        let inserted = connection.execute(
            "
            INSERT INTO provider_continuation_tool_calls (
                continuation_id, provider_tool_index, runtime_call_id
            ) VALUES (?1, ?2, ?3)
            ",
            params![
                &record.continuation_id,
                identity.provider_tool_index,
                &identity.runtime_call_id,
            ],
        )?;
        if inserted != 1 {
            // Returning an ordinary conflict here would let a caller-owned transaction commit a
            // parent envelope with an incomplete Tool identity set. Treat the impossible row
            // count as a repository failure so every normal and fork transaction rolls back.
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    let superseded = if activated_at.is_some() {
        supersede_active_siblings(connection, record, record.created_at)?
    } else {
        0
    };
    Ok(ProviderContinuationStoreOutcome::Inserted { superseded })
}

fn supersede_active_siblings(
    connection: &Connection,
    record: &ProviderContinuationEnvelopeRecord,
    updated_at: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        "
        UPDATE provider_continuations
        SET state = 'superseded', superseded_by = ?1,
            updated_at = MAX(?2, created_at)
        WHERE conversation_id = ?3
          AND assistant_message_id = ?4
          AND run_id = ?5
          AND state = 'active'
          AND activated_at IS NOT NULL
          AND continuation_id != ?1
        ",
        params![
            &record.continuation_id,
            updated_at,
            &record.conversation_id,
            &record.assistant_message_id,
            &record.run_id,
        ],
    )
}

fn load_runtime_tool_identities(
    connection: &Connection,
    continuation_id: &str,
) -> rusqlite::Result<Vec<ProviderContinuationRuntimeToolIdentity>> {
    let mut statement = connection.prepare(
        "
        SELECT provider_tool_index, runtime_call_id
        FROM provider_continuation_tool_calls
        WHERE continuation_id = ?1
        ORDER BY provider_tool_index ASC
        ",
    )?;
    let identities = statement
        .query_map([continuation_id], |row| {
            Ok(ProviderContinuationRuntimeToolIdentity {
                provider_tool_index: row.get(0)?,
                runtime_call_id: row.get(1)?,
            })
        })?
        .collect();
    identities
}

#[allow(dead_code)] // Exact-ref Vault restore; normal hydration uses the ordered conversation list.
pub(crate) fn load(
    connection: &Connection,
    continuation_id: &str,
) -> rusqlite::Result<Option<StoredProviderContinuationRecord>> {
    if !valid_continuation_id(continuation_id) {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation reference".to_string(),
        ));
    }
    connection
        .query_row(
            "
            SELECT continuation_id, schema_version, envelope_version,
                   conversation_id, assistant_message_id, run_id, request_index,
                   assistant_turn_id, assistant_turn_digest, provider_protocol_digest,
                   state, superseded_by, compression, encryption,
                   payload_digest, nonce, ciphertext, decoded_bytes, compressed_bytes,
                   created_at, updated_at, released_at, activated_at
            FROM provider_continuations
            WHERE continuation_id = ?1
            ",
            [continuation_id],
            decode_row,
        )
        .optional()
}

pub(crate) fn list_replayable_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<StoredProviderContinuationRecord>> {
    if !valid_identity(conversation_id) {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation conversation scope".to_string(),
        ));
    }
    promote_recoverable_staged_for_conversation(connection, conversation_id)?;
    let mut statement = connection.prepare(
        "
        SELECT continuation.continuation_id,
               continuation.schema_version,
               continuation.envelope_version,
               continuation.conversation_id,
               continuation.assistant_message_id,
               continuation.run_id,
               continuation.request_index,
               continuation.assistant_turn_id,
               continuation.assistant_turn_digest,
               continuation.provider_protocol_digest,
               continuation.state,
               continuation.superseded_by,
               continuation.compression,
               continuation.encryption,
               continuation.payload_digest,
               continuation.nonce,
               continuation.ciphertext,
               continuation.decoded_bytes,
               continuation.compressed_bytes,
               continuation.created_at,
               continuation.updated_at,
               continuation.released_at,
               continuation.activated_at
        FROM provider_continuations AS continuation
        INNER JOIN messages AS owner
          ON owner.id = continuation.assistant_message_id
         AND owner.conversation_id = continuation.conversation_id
         AND owner.role = 'assistant'
        WHERE continuation.conversation_id = ?1
          AND continuation.state IN ('active', 'superseded')
          AND continuation.activated_at IS NOT NULL
        ORDER BY owner.position ASC,
                 continuation.run_id ASC,
                 continuation.request_index ASC,
                 continuation.created_at ASC,
                 continuation.continuation_id ASC
        ",
    )?;
    let records = statement
        .query_map([conversation_id], decode_row)?
        .collect();
    records
}

pub(crate) fn has_replayable_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<bool> {
    if !valid_identity(conversation_id) {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation conversation scope".to_string(),
        ));
    }
    promote_recoverable_staged_for_conversation(connection, conversation_id)?;
    connection.query_row(
        "
        SELECT EXISTS (
            SELECT 1
            FROM provider_continuations AS continuation
            INNER JOIN messages AS owner
              ON owner.id = continuation.assistant_message_id
             AND owner.conversation_id = continuation.conversation_id
             AND owner.role = 'assistant'
            WHERE continuation.conversation_id = ?1
              AND continuation.state IN ('active', 'superseded')
              AND continuation.activated_at IS NOT NULL
            LIMIT 1
        )
        ",
        [conversation_id],
        |row| row.get(0),
    )
}

pub(crate) fn has_released_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<bool> {
    if !valid_identity(conversation_id) {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation released conversation scope".to_string(),
        ));
    }
    connection.query_row(
        "SELECT EXISTS (
             SELECT 1
             FROM provider_continuations
             WHERE conversation_id = ?1 AND state = 'released'
             LIMIT 1
         )",
        [conversation_id],
        |row| row.get(0),
    )
}

fn load_for_turn(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    request_index: u64,
) -> rusqlite::Result<Option<StoredProviderContinuationRecord>> {
    connection
        .query_row(
            "
            SELECT continuation_id, schema_version, envelope_version,
                   conversation_id, assistant_message_id, run_id, request_index,
                   assistant_turn_id, assistant_turn_digest, provider_protocol_digest,
                   state, superseded_by, compression, encryption,
                   payload_digest, nonce, ciphertext, decoded_bytes, compressed_bytes,
                   created_at, updated_at, released_at, activated_at
            FROM provider_continuations
            WHERE conversation_id = ?1
              AND assistant_message_id = ?2
              AND run_id = ?3
              AND request_index = ?4
            ",
            params![conversation_id, assistant_message_id, run_id, request_index],
            decode_row,
        )
        .optional()
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredProviderContinuationRecord> {
    let schema_version = row.get::<_, u32>(1)?;
    let envelope_version = row.get::<_, u32>(2)?;
    let compression = row.get::<_, Option<String>>(12)?;
    let encryption = row.get::<_, Option<String>>(13)?;
    if schema_version != PROVIDER_CONTINUATION_RECORD_SCHEMA_VERSION
        || envelope_version != PROVIDER_CONTINUATION_ENVELOPE_VERSION
        || compression.as_deref() != Some(PROVIDER_CONTINUATION_COMPRESSION)
        || encryption.as_deref() != Some(PROVIDER_CONTINUATION_ENCRYPTION)
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let state = ProviderContinuationRecordState::parse(&row.get::<_, String>(10)?)?;
    let record = StoredProviderContinuationRecord {
        continuation_id: row.get(0)?,
        conversation_id: row.get(3)?,
        assistant_message_id: row.get(4)?,
        run_id: row.get(5)?,
        request_index: row.get(6)?,
        assistant_turn_id: row.get(7)?,
        assistant_turn_digest: row.get(8)?,
        provider_protocol_digest: row.get(9)?,
        state,
        superseded_by: row.get(11)?,
        payload_digest: row.get(14)?,
        nonce: row.get(15)?,
        ciphertext: row.get(16)?,
        decoded_bytes: row.get(17)?,
        compressed_bytes: row.get(18)?,
        created_at: row.get(19)?,
        updated_at: row.get(20)?,
        released_at: row.get(21)?,
        activated_at: row.get(22)?,
    };
    validate_stored_record(&record)?;
    Ok(record)
}

fn validate_stored_record(record: &StoredProviderContinuationRecord) -> rusqlite::Result<()> {
    let identity_valid = valid_continuation_id(&record.continuation_id)
        && valid_identity(&record.conversation_id)
        && valid_identity(&record.assistant_message_id)
        && valid_identity(&record.run_id)
        && record.assistant_turn_id.starts_with("at1_")
        && valid_sha256_digest(&record.assistant_turn_digest)
        && valid_sha256_digest(&record.provider_protocol_digest)
        && record.created_at >= 0
        && record.updated_at >= record.created_at
        && record
            .released_at
            .is_none_or(|released_at| released_at >= record.created_at)
        && record
            .activated_at
            .is_none_or(|activated_at| activated_at >= record.created_at);
    let payload_valid = match record.state {
        ProviderContinuationRecordState::Active | ProviderContinuationRecordState::Superseded => {
            record
                .payload_digest
                .as_deref()
                .is_some_and(valid_sha256_digest)
                && record
                    .nonce
                    .as_ref()
                    .is_some_and(|nonce| nonce.len() == NONCE_BYTES)
                && record.ciphertext.as_ref().is_some_and(|ciphertext| {
                    ciphertext.len() > 16
                        && ciphertext.len() <= MAX_PROVIDER_CONTINUATION_CIPHERTEXT_BYTES
                })
                && record.decoded_bytes.is_some_and(|bytes| {
                    bytes > 0 && bytes <= MAX_PROVIDER_CONTINUATION_DECODED_BYTES as u64
                })
                && record.compressed_bytes.is_some_and(|bytes| {
                    bytes > 0 && bytes <= MAX_PROVIDER_CONTINUATION_CIPHERTEXT_BYTES as u64
                })
                && record.released_at.is_none()
        }
        ProviderContinuationRecordState::Released => {
            record.payload_digest.is_none()
                && record.nonce.is_none()
                && record.ciphertext.is_none()
                && record.decoded_bytes.is_none()
                && record.compressed_bytes.is_none()
                && record.released_at.is_some()
                && record.activated_at.is_none()
        }
    };
    if !identity_valid || !payload_valid {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

/// Promotes one exact staged envelope after its complete Provider Assistant + ToolCall trace has
/// crossed the Host's durable observer boundary. Promotion is idempotent and never decrypts raw
/// provider state.
pub(crate) fn promote_staged(
    connection: &mut Connection,
    continuation_id: &str,
    promoted_at: i64,
) -> rusqlite::Result<ProviderContinuationPromotionOutcome> {
    if !valid_continuation_id(continuation_id) || promoted_at < 0 {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation promotion".to_string(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(record) = load(&transaction, continuation_id)? else {
        transaction.commit()?;
        return Ok(ProviderContinuationPromotionOutcome::NotFound);
    };
    if record.state == ProviderContinuationRecordState::Released {
        transaction.commit()?;
        return Ok(ProviderContinuationPromotionOutcome::Released);
    }
    if record.activated_at.is_some() {
        transaction.commit()?;
        return Ok(ProviderContinuationPromotionOutcome::AlreadyPromoted);
    }
    let changed = transaction.execute(
        "UPDATE provider_continuations
         SET activated_at = MAX(?2, created_at), updated_at = MAX(?2, created_at)
         WHERE continuation_id = ?1
           AND state IN ('active', 'superseded')
           AND activated_at IS NULL",
        params![continuation_id, promoted_at],
    )?;
    if changed != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    transaction.execute(
        "UPDATE provider_continuations
         SET state = 'superseded', superseded_by = ?1,
             updated_at = MAX(?2, created_at)
         WHERE conversation_id = ?3
           AND assistant_message_id = ?4
           AND run_id = ?5
           AND state = 'active'
           AND activated_at IS NOT NULL
           AND continuation_id != ?1",
        params![
            continuation_id,
            promoted_at,
            &record.conversation_id,
            &record.assistant_message_id,
            &record.run_id,
        ],
    )?;
    transaction.commit()?;
    Ok(ProviderContinuationPromotionOutcome::Promoted)
}

/// Recovers the narrow crash window after a durable trace commit but before explicit promotion.
/// A staged row becomes replayable once its exact owner trace contains any bound Runtime ToolCall.
/// The full Provider Assistant Turn has crossed into visible history at that point even when a
/// multi-call batch crashed before publishing later calls. Exact provider-turn hydration will
/// independently reject an incomplete projection, while legacy projections must still observe
/// the provider boundary. Staged rows with no matching durable call remain invisible orphans.
fn promote_recoverable_staged_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE provider_continuations AS continuation
         SET activated_at = created_at
         WHERE continuation.conversation_id = ?1
           AND continuation.state IN ('active', 'superseded')
           AND continuation.activated_at IS NULL
           AND EXISTS (
               SELECT 1
               FROM conversation_turn_traces AS trace
               WHERE trace.assistant_message_id = continuation.assistant_message_id
                 AND trace.conversation_id = continuation.conversation_id
                 AND trace.run_id = continuation.run_id
           )
           AND EXISTS (
               SELECT 1
               FROM provider_continuation_tool_calls AS expected_call
               INNER JOIN conversation_turn_trace_items AS trace_item
                 ON trace_item.assistant_message_id = continuation.assistant_message_id
                AND trace_item.item_kind = 'tool_call'
                AND json_extract(trace_item.item_json, '$.callId') =
                    expected_call.runtime_call_id
               WHERE expected_call.continuation_id = continuation.continuation_id
           )",
        [conversation_id],
    )
}

#[allow(dead_code)] // Exact-ref Vault release; compaction/edit use caller-owned transactions.
pub(crate) fn release(
    connection: &Connection,
    continuation_id: &str,
    released_at: i64,
) -> rusqlite::Result<ProviderContinuationReleaseOutcome> {
    if !valid_continuation_id(continuation_id) || released_at < 0 {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation release".to_string(),
        ));
    }
    let state = connection
        .query_row(
            "SELECT state FROM provider_continuations WHERE continuation_id = ?1",
            [continuation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(state) = state else {
        return Ok(ProviderContinuationReleaseOutcome::NotFound);
    };
    if state == "released" {
        return Ok(ProviderContinuationReleaseOutcome::AlreadyReleased);
    }
    let changed = connection.execute(
        "
        UPDATE provider_continuations
        SET state = 'released', superseded_by = NULL,
            payload_digest = NULL, nonce = NULL, ciphertext = NULL,
            decoded_bytes = NULL, compressed_bytes = NULL,
            updated_at = MAX(?2, created_at), released_at = MAX(?2, created_at),
            activated_at = NULL
        WHERE continuation_id = ?1 AND state IN ('active', 'superseded')
        ",
        params![continuation_id, released_at],
    )?;
    if changed == 1 {
        Ok(ProviderContinuationReleaseOutcome::Released)
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

pub(crate) fn release_for_messages(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
    released_at: i64,
) -> rusqlite::Result<usize> {
    if message_ids.is_empty() {
        return Ok(0);
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "
        UPDATE provider_continuations
        SET state = 'released', superseded_by = NULL,
            payload_digest = NULL, nonce = NULL, ciphertext = NULL,
            decoded_bytes = NULL, compressed_bytes = NULL,
            updated_at = MAX(?, created_at), released_at = MAX(?, created_at),
            activated_at = NULL
        WHERE conversation_id = ?
          AND assistant_message_id IN ({placeholders})
          AND state IN ('active', 'superseded')
        "
    );
    let mut values = Vec::<rusqlite::types::Value>::with_capacity(message_ids.len() + 3);
    values.push(released_at.into());
    values.push(released_at.into());
    values.push(conversation_id.to_string().into());
    values.extend(message_ids.iter().cloned().map(Into::into));
    connection.execute(&sql, rusqlite::params_from_iter(values))
}

pub(crate) fn release_for_covered_runtime_tool_calls(
    connection: &Connection,
    conversation_id: &str,
    covered_runtime_tool_calls: &[(String, String)],
    released_at: i64,
) -> rusqlite::Result<usize> {
    if covered_runtime_tool_calls.is_empty() {
        return Ok(0);
    }
    if !valid_identity(conversation_id)
        || released_at < 0
        || covered_runtime_tool_calls.len() > MAX_RUNTIME_TOOL_CALLS
        || covered_runtime_tool_calls
            .iter()
            .any(|(run_id, call_id)| !valid_identity(run_id) || !valid_identity(call_id))
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation compaction coverage".to_string(),
        ));
    }
    let covered_predicates = std::iter::repeat_n(
        "(provider_continuations.run_id = ? AND uncovered.runtime_call_id = ?)",
        covered_runtime_tool_calls.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    let sql = format!(
        "
        UPDATE provider_continuations
        SET state = 'released', superseded_by = NULL,
            payload_digest = NULL, nonce = NULL, ciphertext = NULL,
            decoded_bytes = NULL, compressed_bytes = NULL,
            updated_at = MAX(?, created_at), released_at = MAX(?, created_at),
            activated_at = NULL
        WHERE conversation_id = ?
          AND state IN ('active', 'superseded')
          AND EXISTS (
              SELECT 1
              FROM provider_continuation_tool_calls AS any_call
              WHERE any_call.continuation_id = provider_continuations.continuation_id
          )
          AND NOT EXISTS (
              SELECT 1
              FROM provider_continuation_tool_calls AS uncovered
              WHERE uncovered.continuation_id = provider_continuations.continuation_id
                AND NOT ({covered_predicates})
          )
        "
    );
    let mut values = Vec::<rusqlite::types::Value>::with_capacity(
        covered_runtime_tool_calls
            .len()
            .saturating_mul(2)
            .saturating_add(3),
    );
    values.push(released_at.into());
    values.push(released_at.into());
    values.push(conversation_id.to_string().into());
    for (run_id, runtime_call_id) in covered_runtime_tool_calls {
        values.push(run_id.clone().into());
        values.push(runtime_call_id.clone().into());
    }
    connection.execute(&sql, rusqlite::params_from_iter(values))
}

/// Reports whether rolling a summary boundary back across the supplied closed ToolResult set
/// would expose a Provider turn whose encrypted replay payload has already been destroyed.
///
/// A released turn is relevant only when its complete runtime-call set is inside the rollback
/// range. A turn which also owns a later, uncovered call was never eligible for release at this
/// boundary and must not make an otherwise safe rollback fail.
pub(crate) fn has_released_for_covered_runtime_tool_calls(
    connection: &Connection,
    conversation_id: &str,
    covered_runtime_tool_calls: &[(String, String)],
) -> rusqlite::Result<bool> {
    if covered_runtime_tool_calls.is_empty() {
        return Ok(false);
    }
    if !valid_identity(conversation_id)
        || covered_runtime_tool_calls.len() > MAX_RUNTIME_TOOL_CALLS
        || covered_runtime_tool_calls
            .iter()
            .any(|(run_id, call_id)| !valid_identity(run_id) || !valid_identity(call_id))
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation rollback coverage".to_string(),
        ));
    }
    let covered_predicates = std::iter::repeat_n(
        "(provider_continuations.run_id = ? AND uncovered.runtime_call_id = ?)",
        covered_runtime_tool_calls.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    let sql = format!(
        "SELECT EXISTS (
             SELECT 1
             FROM provider_continuations
             WHERE conversation_id = ?
               AND state = 'released'
               AND EXISTS (
                   SELECT 1
                   FROM provider_continuation_tool_calls AS any_call
                   WHERE any_call.continuation_id = provider_continuations.continuation_id
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM provider_continuation_tool_calls AS uncovered
                   WHERE uncovered.continuation_id = provider_continuations.continuation_id
                     AND NOT ({covered_predicates})
               )
             LIMIT 1
         )"
    );
    let mut values = Vec::<rusqlite::types::Value>::with_capacity(
        covered_runtime_tool_calls
            .len()
            .saturating_mul(2)
            .saturating_add(1),
    );
    values.push(conversation_id.to_string().into());
    for (run_id, runtime_call_id) in covered_runtime_tool_calls {
        values.push(run_id.clone().into());
        values.push(runtime_call_id.clone().into());
    }
    connection.query_row(&sql, rusqlite::params_from_iter(values), |row| row.get(0))
}

pub(crate) fn has_released_for_messages(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<bool> {
    if message_ids.is_empty() {
        return Ok(false);
    }
    if !valid_identity(conversation_id)
        || message_ids
            .iter()
            .any(|message_id| !valid_identity(message_id))
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation released-history scope".to_string(),
        ));
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT EXISTS (
             SELECT 1
             FROM provider_continuations
             WHERE conversation_id = ?
               AND assistant_message_id IN ({placeholders})
               AND state = 'released'
             LIMIT 1
         )"
    );
    let mut values = Vec::<rusqlite::types::Value>::with_capacity(message_ids.len() + 1);
    values.push(conversation_id.to_string().into());
    values.extend(message_ids.iter().cloned().map(Into::into));
    connection.query_row(&sql, rusqlite::params_from_iter(values), |row| row.get(0))
}

/// Reports whether a fork's raw-visible message prefix contains any released Provider turn not
/// fully represented by the selected active compaction summary.
///
/// A released row with no Tool-call identities is conservatively visible. Otherwise every child
/// runtime call must be inside `summary_covered_runtime_tool_calls`; partial coverage is unsafe.
pub(crate) fn has_released_for_messages_outside_summary_coverage(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
    summary_covered_runtime_tool_calls: &[(String, String)],
) -> rusqlite::Result<bool> {
    if message_ids.is_empty() {
        return Ok(false);
    }
    if !valid_identity(conversation_id)
        || message_ids
            .iter()
            .any(|message_id| !valid_identity(message_id))
        || summary_covered_runtime_tool_calls.len() > MAX_RUNTIME_TOOL_CALLS
        || summary_covered_runtime_tool_calls
            .iter()
            .any(|(run_id, call_id)| !valid_identity(run_id) || !valid_identity(call_id))
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid provider continuation fork summary coverage".to_string(),
        ));
    }
    if summary_covered_runtime_tool_calls.is_empty() {
        return has_released_for_messages(connection, conversation_id, message_ids);
    }
    let message_placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let covered_predicates = std::iter::repeat_n(
        "(provider_continuations.run_id = ? AND uncovered.runtime_call_id = ?)",
        summary_covered_runtime_tool_calls.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    let sql = format!(
        "SELECT EXISTS (
             SELECT 1
             FROM provider_continuations
             WHERE conversation_id = ?
               AND assistant_message_id IN ({message_placeholders})
               AND state = 'released'
               AND (
                   NOT EXISTS (
                       SELECT 1
                       FROM provider_continuation_tool_calls AS any_call
                       WHERE any_call.continuation_id = provider_continuations.continuation_id
                   )
                   OR EXISTS (
                       SELECT 1
                       FROM provider_continuation_tool_calls AS uncovered
                       WHERE uncovered.continuation_id = provider_continuations.continuation_id
                         AND NOT ({covered_predicates})
                   )
               )
             LIMIT 1
         )"
    );
    let mut values = Vec::<rusqlite::types::Value>::with_capacity(
        1usize
            .saturating_add(message_ids.len())
            .saturating_add(summary_covered_runtime_tool_calls.len().saturating_mul(2)),
    );
    values.push(conversation_id.to_string().into());
    values.extend(message_ids.iter().cloned().map(Into::into));
    for (run_id, runtime_call_id) in summary_covered_runtime_tool_calls {
        values.push(run_id.clone().into());
        values.push(runtime_call_id.clone().into());
    }
    connection.query_row(&sql, rusqlite::params_from_iter(values), |row| row.get(0))
}

pub(crate) fn delete_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "DELETE FROM provider_continuations WHERE conversation_id = ?1",
        [conversation_id],
    )
}
