//! Workflow authoring only: graph definitions are persisted without creating executable Agents.
use crate::storage::now_ms;
use crate::workflow::{Definition, InvalidRecord, InvalidRecordReason, Record, Request, Response};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::collections::HashSet;

const MAX_SAFE_REVISION: i64 = 9_007_199_254_740_991;
const MAX_DEFINITIONS: usize = 1_000;
const MAX_CATALOG_BYTES: i64 = 16 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    Conflict(String),
    Storage(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Conflict(message) | Self::Storage(message) => {
                formatter.write_str(message)
            }
        }
    }
}
impl std::error::Error for Error {}

fn storage_error(_: impl std::fmt::Display) -> Error {
    Error::Storage("Workflow storage operation failed".into())
}

/// Validation and mutation share a transaction, without depending on subagent templates.
/// Model identities come from the service's credential-aware execution projection, never a
/// second SQL-only interpretation of which configured models are usable.
pub fn request(
    connection: &mut Connection,
    request: Request,
    available_models: &HashSet<String>,
) -> Result<Response, Error> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let mut response = Response::default();
    match request {
        Request::List => {}
        Request::Validate { definition } => {
            response.issues = definition
                .validate(available_models)
                .map_err(Error::Invalid)?;
        }
        Request::Save {
            definition,
            expected_revision,
        } => {
            management::publish(
                &transaction,
                &definition,
                expected_revision,
                None,
                None,
                available_models,
            )?;
        }
        Request::SaveWithUsage {
            definition,
            expected_revision,
            expected_usage_revision,
            expected_draft_revision,
        } => {
            management::publish(
                &transaction,
                &definition,
                expected_revision,
                expected_usage_revision.as_deref(),
                expected_draft_revision,
                available_models,
            )?;
        }
        Request::Delete {
            id,
            expected_revision,
        } => {
            validate_id(&id)?;
            if management::template_in_use(&transaction, &id)? {
                return Err(Error::Conflict("workflow_template_in_use".into()));
            }
            let expected = revision_to_sql(expected_revision)?;
            if expected == 0 {
                return Err(Error::Invalid(
                    "Deleting a workflow requires its current revision".into(),
                ));
            }
            let changed = transaction
                .execute(
                    "DELETE FROM workflow_definitions WHERE workflow_id = ?1 AND revision = ?2",
                    params![id, expected],
                )
                .map_err(storage_error)?;
            if changed != 1 {
                return Err(revision_conflict());
            }
        }
        Request::SetEnabled {
            id,
            enabled,
            expected_revision,
        } => {
            set_enabled(
                &transaction,
                &id,
                enabled,
                expected_revision,
                available_models,
            )?;
        }
        Request::Manage(request) => {
            response.affected_conversation_ids =
                management::request(&transaction, request, available_models)?;
        }
    }
    (response.records, response.invalid_records) = list(&transaction, available_models)?;
    response.instances = management::list_instances(&transaction)?;
    response.usages = management::list_usages(&transaction)?;
    (response.drafts, response.invalid_drafts) =
        management::list_drafts(&transaction, available_models)?;
    transaction.commit().map_err(storage_error)?;
    Ok(response)
}

mod management;

fn list(
    connection: &Connection,
    available_models: &HashSet<String>,
) -> Result<(Vec<Record>, Vec<InvalidRecord>), Error> {
    let (count, bytes) = catalog_size(connection)?;
    if count > MAX_DEFINITIONS || bytes > MAX_CATALOG_BYTES {
        return Err(Error::Storage(
            "Workflow catalog exceeds its storage limit".into(),
        ));
    }
    let mut statement = connection
        .prepare(
            "SELECT workflow_id, definition_json, revision, updated_at, enabled FROM workflow_definitions
         ORDER BY updated_at DESC, workflow_id ASC",
        )
        .map_err(storage_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })
        .map_err(storage_error)?;
    let mut records = vec![];
    let mut invalid_records = vec![];
    for row in rows {
        let (id, json, revision, updated_at, enabled) = row.map_err(storage_error)?;
        if !(1..=MAX_SAFE_REVISION).contains(&revision) || updated_at < 0 {
            return Err(Error::Storage(
                "Invalid persisted workflow record metadata".into(),
            ));
        }
        match parse_persisted_definition(&json, &id, available_models) {
            Ok((definition, issues)) => records.push(Record {
                definition,
                enabled: enabled && issues.is_empty(),
                revision: revision as u64,
                updated_at,
                issues,
            }),
            Err(reason) => invalid_records.push(InvalidRecord {
                name: recovery_display_name(&json, &id),
                id,
                revision: revision as u64,
                updated_at,
                reason,
            }),
        }
    }
    Ok((records, invalid_records))
}

/// Invalid graph bytes stay untouched and are never exposed as an editable placeholder.
/// Recovery metadata lets callers explicitly remove a record using its original revision.
fn parse_persisted_definition(
    json: &str,
    id: &str,
    models: &HashSet<String>,
) -> Result<(Definition, Vec<crate::workflow::Issue>), InvalidRecordReason> {
    let definition: Definition =
        serde_json::from_str(json).map_err(|_| InvalidRecordReason::IncompatibleDefinition)?;
    if definition.id != id {
        return Err(InvalidRecordReason::InvalidDefinition);
    }
    let issues = definition
        .validate(models)
        .map_err(|_| InvalidRecordReason::InvalidDefinition)?;
    Ok((definition, issues))
}

fn recovery_display_name(json: &str, id: &str) -> String {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|value| {
            value
                .get("name")
                .and_then(|name| name.as_str())
                .map(str::to_owned)
        })
        .filter(|name| {
            !name.trim().is_empty() && name.len() <= 512 && !name.chars().any(char::is_control)
        })
        .map(|name| name.trim().to_owned())
        .unwrap_or_else(|| id.to_owned())
}

fn save(
    connection: &Connection,
    definition: &Definition,
    expected_revision: u64,
    valid: bool,
) -> Result<(), Error> {
    let expected = revision_to_sql(expected_revision)?;
    let current: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT revision, updated_at, length(CAST(definition_json AS BLOB))
             FROM workflow_definitions WHERE workflow_id = ?1",
            [&definition.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage_error)?;
    if current.as_ref().map(|row| row.0).unwrap_or(0) != expected {
        return Err(revision_conflict());
    }
    let next = next_revision(expected)?;
    let json = serde_json::to_string(definition).map_err(storage_error)?;
    let (count, bytes) = catalog_size(connection)?;
    let replaced_bytes = current.as_ref().map(|row| row.2).unwrap_or(0);
    if bytes - replaced_bytes + json.len() as i64 > MAX_CATALOG_BYTES {
        return Err(Error::Invalid(
            "Workflow catalog exceeds 16 MiB; shorten or delete a definition before saving".into(),
        ));
    }
    let timestamp = now_ms().max(
        current
            .as_ref()
            .map(|row| row.1.saturating_add(1))
            .unwrap_or(0),
    );
    if current.is_some() {
        let changed = connection.execute(
            "UPDATE workflow_definitions SET definition_json = ?1, revision = ?2, updated_at = ?3,
             enabled = CASE WHEN ?6 THEN enabled ELSE 0 END
             WHERE workflow_id = ?4 AND revision = ?5",
            params![json, next, timestamp, definition.id, expected, valid],
        ).map_err(storage_error)?;
        if changed != 1 {
            return Err(revision_conflict());
        }
    } else {
        if count >= MAX_DEFINITIONS {
            return Err(Error::Invalid(
                "At most 1000 workflow definitions can be saved".into(),
            ));
        }
        connection.execute(
            "INSERT INTO workflow_definitions (workflow_id, definition_json, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4)", params![definition.id, json, next, timestamp],
        ).map_err(storage_error)?;
    }
    Ok(())
}

fn set_enabled(
    connection: &Connection,
    id: &str,
    enabled: bool,
    expected_revision: u64,
    available_models: &HashSet<String>,
) -> Result<(), Error> {
    validate_id(id)?;
    let expected = revision_to_sql(expected_revision)?;
    if expected == 0 {
        return Err(Error::Invalid(
            "Changing workflow availability requires its current revision".into(),
        ));
    }
    let current: Option<(String, i64, i64)> = connection
        .query_row(
            "SELECT definition_json, revision, updated_at FROM workflow_definitions WHERE workflow_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage_error)?;
    let Some((json, revision, updated_at)) = current else {
        return Err(revision_conflict());
    };
    if revision != expected {
        return Err(revision_conflict());
    }
    let definition: Definition = serde_json::from_str(&json).map_err(storage_error)?;
    if definition.id != id || updated_at < 0 {
        return Err(Error::Storage("Invalid persisted workflow record".into()));
    }
    let issues = definition
        .validate(available_models)
        .map_err(|_| Error::Storage("Invalid persisted workflow graph".into()))?;
    if !issues.is_empty() {
        return Err(Error::Invalid(
            "Save a valid workflow before changing its availability".into(),
        ));
    }
    let next = next_revision(expected)?;
    let timestamp = now_ms().max(updated_at.saturating_add(1));
    let changed = connection
        .execute(
            "UPDATE workflow_definitions SET enabled = ?1, revision = ?2, updated_at = ?3
             WHERE workflow_id = ?4 AND revision = ?5",
            params![enabled, next, timestamp, id, expected],
        )
        .map_err(storage_error)?;
    if changed != 1 {
        return Err(revision_conflict());
    }
    // Availability changes do not alter graph content; keep an existing draft based
    // on this exact published revision publishable after reopening the editor.
    connection.execute("UPDATE workflow_editing_drafts SET base_revision=?1 WHERE template_id=?2 AND base_revision=?3",params![next,id,expected]).map_err(storage_error)?;
    Ok(())
}

fn catalog_size(connection: &Connection) -> Result<(usize, i64), Error> {
    connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(definition_json AS BLOB))), 0)
         FROM workflow_definitions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(storage_error)
}

fn revision_conflict() -> Error {
    Error::Conflict("Workflow changed or was deleted; reload before changing it".into())
}
fn next_revision(revision: i64) -> Result<i64, Error> {
    revision
        .checked_add(1)
        .filter(|next| *next <= MAX_SAFE_REVISION)
        .ok_or_else(|| Error::Invalid("Workflow revision exhausted".into()))
}
fn revision_to_sql(revision: u64) -> Result<i64, Error> {
    i64::try_from(revision)
        .ok()
        .filter(|revision| *revision <= MAX_SAFE_REVISION)
        .ok_or_else(|| Error::Invalid("Invalid workflow revision".into()))
}
fn validate_id(id: &str) -> Result<(), Error> {
    if id.trim().is_empty() || id.len() > 256 {
        return Err(Error::Invalid("Invalid workflow identifier".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod management_tests;
