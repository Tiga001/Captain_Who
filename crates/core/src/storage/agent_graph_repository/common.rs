use crate::{AgentGraphError, AGENT_GRAPH_SCHEMA_VERSION};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

pub(super) const MAX_ID_BYTES: usize = 128;
pub(super) const MAX_REQUEST_ID_BYTES: usize = 256;
pub(super) const MAX_TASK_NAME_BYTES: usize = 256;
pub(super) const MAX_TASK_PATH_BYTES: usize = 2_048;
pub(super) const MAX_UNBOUND_MAILBOX_MESSAGES_PER_RECIPIENT: u64 = 1_024;
pub(super) const MAX_UNBOUND_MAILBOX_BYTES_PER_RECIPIENT: u64 = 16 * 1_024 * 1_024;
pub(super) const MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES_PER_RECIPIENT: u64 = 960;
pub(super) const MAX_UNBOUND_ORDINARY_MAILBOX_BYTES_PER_RECIPIENT: u64 = 15 * 1_024 * 1_024;
pub(super) const MAX_TERMINAL_ERROR_BYTES: usize = crate::AGENT_RESULT_TERMINAL_ERROR_MAX_BYTES;
pub(super) const MAX_RESULT_ARTIFACTS: usize = 256;
pub(super) const MAX_PROJECT_BATCH: usize = 1_024;
pub(super) const WAKE_LEASE_DURATION_MS: i64 = 60_000;

pub(super) fn immediate(connection: &mut Connection) -> Result<Transaction<'_>, AgentGraphError> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(write_error)
}

pub(super) fn validate_schema_version(value: i64) -> Result<(), AgentGraphError> {
    if value == i64::from(AGENT_GRAPH_SCHEMA_VERSION) {
        Ok(())
    } else {
        Err(corrupt("unsupported Agent graph record schema version"))
    }
}

pub(super) fn validate_id(field: &'static str, value: &str) -> Result<(), AgentGraphError> {
    validate_bounded_text(field, value, 1, MAX_ID_BYTES)
}

pub(super) fn validate_request_id(value: &str) -> Result<(), AgentGraphError> {
    validate_bounded_text("request_id", value, 1, MAX_REQUEST_ID_BYTES)
}

pub(super) fn validate_trimmed(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), AgentGraphError> {
    validate_bounded_text(field, value, 1, maximum)?;
    if value.trim() != value {
        return Err(invalid(
            field,
            "must not contain leading or trailing whitespace",
        ));
    }
    Ok(())
}

pub(super) fn validate_identity_task_name(value: &str) -> Result<(), AgentGraphError> {
    validate_trimmed("task_name", value, MAX_TASK_NAME_BYTES)?;
    if value.contains('/') || value.chars().any(char::is_control) {
        return Err(invalid(
            "task_name",
            "must be one control-free Agent task-path segment",
        ));
    }
    Ok(())
}

pub(super) fn validate_bounded_text(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), AgentGraphError> {
    if value.len() < minimum || value.len() > maximum || value.contains('\0') {
        return Err(invalid(
            field,
            format!("must contain {minimum}..={maximum} bytes and no NUL"),
        ));
    }
    Ok(())
}

pub(super) fn validate_message_content(
    field: &'static str,
    value: &str,
) -> Result<(), AgentGraphError> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(invalid(field, "must be non-empty and NUL-free"));
    }
    Ok(())
}

pub(super) fn validate_time(value: i64) -> Result<(), AgentGraphError> {
    if value < 0 {
        Err(invalid("timestamp", "must be non-negative"))
    } else {
        Ok(())
    }
}

pub(super) fn validate_revision(value: u64) -> Result<(), AgentGraphError> {
    if value == 0 || value > i64::MAX as u64 {
        Err(invalid("revision", "must be a positive SQLite integer"))
    } else {
        Ok(())
    }
}

pub(super) fn revision_to_sql(value: u64) -> Result<i64, AgentGraphError> {
    i64::try_from(value).map_err(|_| invalid("revision", "is outside SQLite integer range"))
}

pub(super) fn positive_u64(value: i64, label: &str) -> Result<u64, AgentGraphError> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| corrupt(format!("{label} is not positive")))
}

pub(super) fn decode_bool(value: i64, label: &str) -> Result<bool, AgentGraphError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(corrupt(format!("{label} is not boolean"))),
    }
}

pub(crate) fn stable_fact_id(prefix: &str, parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{prefix}-{:x}", digest.finalize())
}

pub(super) fn invalid(field: &'static str, reason: impl Into<String>) -> AgentGraphError {
    AgentGraphError::InvalidInput {
        field,
        reason: reason.into(),
    }
}

pub(super) fn conflict(reason: impl Into<String>) -> AgentGraphError {
    AgentGraphError::Conflict(reason.into())
}

pub(super) fn corrupt(reason: impl Into<String>) -> AgentGraphError {
    AgentGraphError::CorruptRecord(reason.into())
}

pub(super) fn read_error(error: rusqlite::Error) -> AgentGraphError {
    match error {
        rusqlite::Error::InvalidColumnType(..)
        | rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..) => {
            corrupt("persisted Agent graph record has an invalid type or range")
        }
        _ => AgentGraphError::StorageUnavailable("database read failed".to_string()),
    }
}

pub(super) fn write_error(_: rusqlite::Error) -> AgentGraphError {
    AgentGraphError::StorageUnavailable("database transaction failed".to_string())
}
