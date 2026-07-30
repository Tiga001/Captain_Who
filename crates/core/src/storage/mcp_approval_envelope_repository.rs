use crate::storage::models::McpApprovalEnvelopeRecord;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const MAX_INVOCATION_ID_BYTES: usize = 256;
const MAX_ACTION_ID_BYTES: usize = 2_048;
const MAX_ENVELOPE_JSON_BYTES: usize = 256 * 1_024;
const NONCE_BYTES: usize = 12;
const MAX_CIPHERTEXT_BYTES: usize = 64 * 1024 + 16;
const SHA256_HEX_BYTES: usize = 64;

/// The only JSON object admitted to SQLite's `envelope_json` column.
///
/// Repository callers cannot provide arbitrary JSON. In particular, the full authenticated
/// metadata, argument digest, and credential reference have no field in this DTO.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredMcpApprovalCiphertext {
    nonce_base64: String,
    ciphertext_base64: String,
}

impl std::fmt::Debug for StoredMcpApprovalCiphertext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StoredMcpApprovalCiphertext([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpApprovalEnvelopeStoreOutcome {
    Inserted,
    Idempotent,
    Conflict,
}

fn valid_identifier(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.len() == SHA256_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_canonical_base64(value: &str, maximum_bytes: usize) -> Option<Vec<u8>> {
    if value.is_empty() || value.len() > MAX_ENVELOPE_JSON_BYTES {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(value).ok()?;
    if decoded.is_empty()
        || decoded.len() > maximum_bytes
        || URL_SAFE_NO_PAD.encode(&decoded) != value
    {
        return None;
    }
    Some(decoded)
}

fn validate_record(record: &McpApprovalEnvelopeRecord) -> rusqlite::Result<()> {
    let nonce = decode_canonical_base64(&record.nonce_base64, NONCE_BYTES);
    let ciphertext = decode_canonical_base64(&record.ciphertext_base64, MAX_CIPHERTEXT_BYTES);
    if !valid_identifier(&record.invocation_id, MAX_INVOCATION_ID_BYTES)
        || !valid_identifier(&record.action_id, MAX_ACTION_ID_BYTES)
        || record.envelope_version <= 0
        || nonce
            .as_ref()
            .is_none_or(|nonce| nonce.len() != NONCE_BYTES)
        || ciphertext
            .as_ref()
            .is_none_or(|ciphertext| ciphertext.len() <= 16)
        || !valid_digest(&record.aad_digest)
        || record.created_at < 0
        || record.expires_at <= record.created_at
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "invalid authenticated MCP approval envelope metadata".to_string(),
        ));
    }
    Ok(())
}

fn encode_ciphertext(record: &McpApprovalEnvelopeRecord) -> rusqlite::Result<String> {
    let encoded = serde_json::to_string(&StoredMcpApprovalCiphertext {
        nonce_base64: record.nonce_base64.clone(),
        ciphertext_base64: record.ciphertext_base64.clone(),
    })
    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    if encoded.len() > MAX_ENVELOPE_JSON_BYTES {
        return Err(rusqlite::Error::InvalidParameterName(
            "authenticated MCP approval ciphertext exceeds its storage budget".to_string(),
        ));
    }
    Ok(encoded)
}

fn decode_record(
    invocation_id: String,
    action_id: String,
    envelope_version: i64,
    envelope_json: String,
    aad_digest: String,
    created_at: i64,
    expires_at: i64,
) -> rusqlite::Result<McpApprovalEnvelopeRecord> {
    if envelope_json.is_empty() || envelope_json.len() > MAX_ENVELOPE_JSON_BYTES {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let ciphertext =
        serde_json::from_str::<StoredMcpApprovalCiphertext>(&envelope_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    let record = McpApprovalEnvelopeRecord {
        invocation_id,
        action_id,
        envelope_version,
        nonce_base64: ciphertext.nonce_base64,
        ciphertext_base64: ciphertext.ciphertext_base64,
        aad_digest,
        created_at,
        expires_at,
    };
    validate_record(&record)?;
    Ok(record)
}

pub fn store(
    connection: &Connection,
    record: &McpApprovalEnvelopeRecord,
) -> rusqlite::Result<McpApprovalEnvelopeStoreOutcome> {
    validate_record(record)?;
    let envelope_json = encode_ciphertext(record)?;
    let inserted = connection.execute(
        "
        INSERT INTO mcp_approval_payload_envelopes (
            invocation_id,
            action_id,
            envelope_version,
            envelope_json,
            aad_digest,
            created_at,
            expires_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT DO NOTHING
        ",
        params![
            &record.invocation_id,
            &record.action_id,
            record.envelope_version,
            &envelope_json,
            &record.aad_digest,
            record.created_at,
            record.expires_at,
        ],
    )?;
    if inserted == 1 {
        return Ok(McpApprovalEnvelopeStoreOutcome::Inserted);
    }
    let existing = load(connection, &record.invocation_id)?;
    if existing.as_ref() == Some(record) {
        Ok(McpApprovalEnvelopeStoreOutcome::Idempotent)
    } else {
        Ok(McpApprovalEnvelopeStoreOutcome::Conflict)
    }
}

pub fn load(
    connection: &Connection,
    invocation_id: &str,
) -> rusqlite::Result<Option<McpApprovalEnvelopeRecord>> {
    connection
        .query_row(
            "
            SELECT invocation_id, action_id, envelope_version, envelope_json,
                   aad_digest, created_at, expires_at
            FROM mcp_approval_payload_envelopes
            WHERE invocation_id = ?1
            ",
            [invocation_id],
            |row| {
                decode_record(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                )
            },
        )
        .optional()
}

/// Atomically removes and returns one envelope.
///
/// The immediate transaction serializes concurrent approval attempts. Exactly one caller can
/// receive the ciphertext; subsequent calls observe `None`.
pub fn take(
    connection: &mut Connection,
    invocation_id: &str,
) -> rusqlite::Result<Option<McpApprovalEnvelopeRecord>> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let record = load(&transaction, invocation_id)?;
    if record.is_some() {
        let deleted = transaction.execute(
            "DELETE FROM mcp_approval_payload_envelopes WHERE invocation_id = ?1",
            [invocation_id],
        )?;
        if deleted != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    transaction.commit()?;
    Ok(record)
}

pub fn delete(connection: &Connection, invocation_id: &str) -> rusqlite::Result<bool> {
    Ok(connection.execute(
        "DELETE FROM mcp_approval_payload_envelopes WHERE invocation_id = ?1",
        [invocation_id],
    )? == 1)
}

pub fn list_expired(
    connection: &Connection,
    now_ms: i64,
    limit: usize,
) -> rusqlite::Result<Vec<McpApprovalEnvelopeRecord>> {
    let limit = i64::try_from(limit.min(1_024)).unwrap_or(1_024);
    let mut statement = connection.prepare(
        "
        SELECT invocation_id, action_id, envelope_version, envelope_json,
               aad_digest, created_at, expires_at
        FROM mcp_approval_payload_envelopes
        WHERE expires_at <= ?1
        ORDER BY expires_at ASC, invocation_id ASC
        LIMIT ?2
        ",
    )?;
    let records = statement
        .query_map(params![now_ms, limit], |row| {
            decode_record(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            )
        })?
        .collect();
    records
}

pub fn delete_expired(connection: &Connection, now_ms: i64) -> rusqlite::Result<usize> {
    connection.execute(
        "DELETE FROM mcp_approval_payload_envelopes WHERE expires_at <= ?1",
        [now_ms],
    )
}

/// Removes envelopes whose safe pending row no longer exists or no longer represents an MCP call.
pub fn delete_orphans(connection: &Connection) -> rusqlite::Result<usize> {
    connection.execute(
        "
        DELETE FROM mcp_approval_payload_envelopes
        WHERE NOT EXISTS (
            SELECT 1
            FROM agent_pending_actions pending
            WHERE pending.action_id = mcp_approval_payload_envelopes.action_id
              AND pending.action_type = 'mcp_tool_call'
              AND pending.status IN ('pending', 'approved', 'executing')
        )
        ",
        [],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    const CANARY: &str = "MCP_DATABASE_PLAINTEXT_CANARY";

    fn record(invocation_id: &str, action_id: &str) -> McpApprovalEnvelopeRecord {
        McpApprovalEnvelopeRecord {
            invocation_id: invocation_id.to_string(),
            action_id: action_id.to_string(),
            envelope_version: 2,
            nonce_base64: URL_SAFE_NO_PAD.encode([7_u8; NONCE_BYTES]),
            ciphertext_base64: URL_SAFE_NO_PAD.encode([9_u8; 32]),
            aad_digest: "a".repeat(64),
            created_at: 1,
            expires_at: 2,
        }
    }

    #[test]
    fn stores_and_atomically_consumes_only_authenticated_envelopes() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let record = record("invocation-1", "action-1");
        assert_eq!(
            store(&connection, &record).unwrap(),
            McpApprovalEnvelopeStoreOutcome::Inserted
        );
        assert_eq!(
            store(&connection, &record).unwrap(),
            McpApprovalEnvelopeStoreOutcome::Idempotent
        );
        assert_eq!(take(&mut connection, "invocation-1").unwrap(), Some(record));
        assert_eq!(take(&mut connection, "invocation-1").unwrap(), None);
    }

    #[test]
    fn stored_ciphertext_debug_is_constant_and_redacted() {
        let ciphertext = StoredMcpApprovalCiphertext {
            nonce_base64: CANARY.to_string(),
            ciphertext_base64: CANARY.to_string(),
        };

        let rendered = format!("{ciphertext:?}");
        assert_eq!(rendered, "StoredMcpApprovalCiphertext([REDACTED])");
        assert!(!rendered.contains(CANARY));
    }

    #[test]
    fn rejects_plaintext_sized_or_invalid_metadata_and_cleans_orphans() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let mut invalid = record("invocation-1", "action-1");
        invalid.ciphertext_base64 = CANARY.repeat(MAX_ENVELOPE_JSON_BYTES);
        assert!(store(&connection, &invalid).is_err());

        let record = record("invocation-2", "action-2");
        store(&connection, &record).unwrap();
        let raw_envelope = connection
            .query_row(
                "SELECT COALESCE(group_concat(envelope_json, ''), '') \
                 FROM mcp_approval_payload_envelopes",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(!raw_envelope.contains(CANARY));
        assert!(!raw_envelope.contains("\"aad\""));
        assert!(!raw_envelope.contains("argumentsDigest"));
        assert!(!raw_envelope.contains("credentialRef"));
        assert!(!raw_envelope.contains("invocationId"));
        let persisted_keys = serde_json::from_str::<serde_json::Value>(&raw_envelope)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            persisted_keys,
            std::collections::BTreeSet::from([
                "ciphertextBase64".to_string(),
                "nonceBase64".to_string(),
            ])
        );
        assert_eq!(delete_orphans(&connection).unwrap(), 1);
    }

    #[test]
    fn rejects_legacy_or_extended_envelope_json_on_read() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let record = record("invocation-legacy", "action-legacy");
        let unsafe_json = serde_json::json!({
            "nonceBase64": record.nonce_base64,
            "ciphertextBase64": record.ciphertext_base64,
            "aad": {"argumentsDigest": CANARY},
            "credentialRef": CANARY,
        })
        .to_string();
        connection
            .execute(
                "
                INSERT INTO mcp_approval_payload_envelopes (
                    invocation_id, action_id, envelope_version, envelope_json,
                    aad_digest, created_at, expires_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ",
                params![
                    &record.invocation_id,
                    &record.action_id,
                    record.envelope_version,
                    unsafe_json,
                    &record.aad_digest,
                    record.created_at,
                    record.expires_at,
                ],
            )
            .unwrap();

        assert!(load(&connection, &record.invocation_id).is_err());
    }
}
