//! Workflow authoring only: graph definitions are persisted without creating executable Agents.
use crate::storage::now_ms;
use crate::workflow::{Definition, Record, Request, Response};
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

/// Validation and mutation share a transaction, including the template availability snapshot.
/// Model identities come from the service's credential-aware execution projection, never a
/// second SQL-only interpretation of which configured models are usable.
pub fn request(
    connection: &mut Connection,
    request: Request,
    available_models: &HashSet<String>,
) -> Result<Response, Error> {
    let behavior = match &request {
        Request::Save { .. } | Request::Delete { .. } => TransactionBehavior::Immediate,
        _ => TransactionBehavior::Deferred,
    };
    let transaction = connection
        .transaction_with_behavior(behavior)
        .map_err(storage_error)?;
    let templates = template_ids(&transaction)?;
    let response = match request {
        Request::List => Response {
            records: list(&transaction, &templates, available_models)?,
            issues: vec![],
        },
        Request::Validate { definition } => Response {
            records: vec![],
            issues: definition
                .validate(&templates, available_models)
                .map_err(Error::Invalid)?,
        },
        Request::Save {
            definition,
            expected_revision,
        } => {
            // Missing templates, incomplete tasks and invalid semantic rules remain editable drafts.
            definition
                .validate(&templates, available_models)
                .map_err(Error::Invalid)?;
            save(&transaction, &definition, expected_revision)?;
            Response {
                records: list(&transaction, &templates, available_models)?,
                issues: vec![],
            }
        }
        Request::Delete {
            id,
            expected_revision,
        } => {
            validate_id(&id)?;
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
            Response {
                records: list(&transaction, &templates, available_models)?,
                issues: vec![],
            }
        }
    };
    transaction.commit().map_err(storage_error)?;
    Ok(response)
}

fn template_ids(connection: &Connection) -> Result<HashSet<String>, Error> {
    let mut statement = connection
        .prepare("SELECT template_id FROM agent_templates")
        .map_err(storage_error)?;
    let rows = statement
        .query_map([], |row| row.get(0))
        .map_err(storage_error)?;
    rows.collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(storage_error)
}

fn list(
    connection: &Connection,
    templates: &HashSet<String>,
    available_models: &HashSet<String>,
) -> Result<Vec<Record>, Error> {
    let (count, bytes) = catalog_size(connection)?;
    if count > MAX_DEFINITIONS || bytes > MAX_CATALOG_BYTES {
        return Err(Error::Storage(
            "Workflow catalog exceeds its storage limit".into(),
        ));
    }
    let mut statement = connection
        .prepare(
            "SELECT workflow_id, definition_json, revision, updated_at FROM workflow_definitions
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
            ))
        })
        .map_err(storage_error)?;
    rows.map(|row| {
        let (id, json, revision, updated_at) = row.map_err(storage_error)?;
        let definition: Definition = serde_json::from_str(&json).map_err(storage_error)?;
        if definition.id != id || !(1..=MAX_SAFE_REVISION).contains(&revision) || updated_at < 0 {
            return Err(Error::Storage("Invalid persisted workflow record".into()));
        }
        let issues = definition
            .validate(templates, available_models)
            .map_err(|_| Error::Storage("Invalid persisted workflow graph".into()))?;
        Ok(Record {
            definition,
            revision: revision as u64,
            updated_at,
            issues,
        })
    })
    .collect()
}

fn save(
    connection: &Connection,
    definition: &Definition,
    expected_revision: u64,
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
    let next = expected
        .checked_add(1)
        .filter(|next| *next <= MAX_SAFE_REVISION)
        .ok_or_else(|| Error::Invalid("Workflow revision exhausted".into()))?;
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
            "UPDATE workflow_definitions SET definition_json = ?1, revision = ?2, updated_at = ?3
             WHERE workflow_id = ?4 AND revision = ?5",
            params![json, next, timestamp, definition.id, expected],
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
    Error::Conflict("Workflow changed or was deleted; reload before saving or deleting".into())
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
