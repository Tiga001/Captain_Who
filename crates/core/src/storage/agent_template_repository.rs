//! Strict SQLite persistence for global Agent templates and project availability bindings.
//!
//! A template keeps only a reference to the existing model-settings identity. The reference is
//! deliberately not a foreign key to `models`: model settings are replaced as one atomic catalog,
//! while a disabled, renamed, or removed model must leave the template visible and explicitly
//! unavailable at spawn resolution time.

use crate::storage::now_ms;
use crate::{
    AgentTemplateError, AgentTemplateRecord, CreateAgentTemplateInput, UpdateAgentTemplateInput,
    AGENT_GRAPH_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

const MAX_TEMPLATE_ID_BYTES: usize = 128;
const MAX_MACHINE_KEY_BYTES: usize = 64;
const MAX_NAME_BYTES: usize = 256;
const MAX_DESCRIPTION_BYTES: usize = 4 * 1024;
const MAX_INSTRUCTIONS_BYTES: usize = 64 * 1024;
const MAX_MODEL_CONFIG_ID_BYTES: usize = 512;
const MAX_SQLITE_REVISION: u64 = 9_223_372_036_854_775_807;
const MAX_PROJECT_TEMPLATES: u32 = 32;

#[derive(Debug)]
struct StoredTemplateRow {
    template_id: String,
    schema_version: i64,
    machine_key: String,
    name: String,
    description: String,
    instructions: String,
    model_config_id: String,
    enabled: i64,
    revision: i64,
    created_at: i64,
    updated_at: i64,
}

/// Creates one template in a transaction and returns the canonical stored record.
pub fn create_template(
    connection: &mut Connection,
    input: &CreateAgentTemplateInput,
) -> Result<AgentTemplateRecord, AgentTemplateError> {
    validate_create_input(input)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_failure)?;

    if template_id_exists(&transaction, &input.template_id)? {
        return Err(AgentTemplateError::InvalidInput {
            field: "template_id",
            reason: "already exists".to_string(),
        });
    }
    ensure_unique_machine_key(&transaction, &input.machine_key, None)?;
    ensure_unique_name(&transaction, &input.name, None)?;

    let timestamp = now_ms();
    transaction
        .execute(
            "INSERT INTO agent_templates (
                template_id, schema_version, machine_key, name, description,
                instructions, model_config_id, enabled, revision, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9)",
            params![
                &input.template_id,
                i64::from(AGENT_GRAPH_SCHEMA_VERSION),
                &input.machine_key,
                &input.name,
                &input.description,
                &input.instructions,
                &input.model_config_id,
                input.enabled,
                timestamp,
            ],
        )
        .map_err(storage_failure)?;
    let stored = query_template(&transaction, &input.template_id)?
        .ok_or_else(|| corrupt("created template could not be read back"))?;
    transaction.commit().map_err(storage_failure)?;
    Ok(stored)
}

pub fn list_templates(
    connection: &Connection,
    include_disabled: bool,
) -> Result<Vec<AgentTemplateRecord>, AgentTemplateError> {
    let sql = if include_disabled {
        "SELECT template_id, schema_version, machine_key, name, description,
                instructions, model_config_id, enabled, revision, created_at, updated_at
         FROM agent_templates
         ORDER BY created_at ASC, template_id ASC"
    } else {
        "SELECT template_id, schema_version, machine_key, name, description,
                instructions, model_config_id, enabled, revision, created_at, updated_at
         FROM agent_templates
         WHERE enabled = 1
         ORDER BY created_at ASC, template_id ASC"
    };
    let mut statement = connection.prepare(sql).map_err(storage_failure)?;
    let rows = statement
        .query_map([], read_stored_row)
        .map_err(storage_failure)?;
    let mut stored_rows = Vec::new();
    for row in rows {
        stored_rows.push(row.map_err(read_failure)?);
    }
    drop(statement);
    stored_rows
        .into_iter()
        .map(|row| decode_row(connection, row))
        .collect()
}

pub fn get_template(
    connection: &Connection,
    template_id: &str,
) -> Result<AgentTemplateRecord, AgentTemplateError> {
    validate_opaque_id("template_id", template_id, MAX_TEMPLATE_ID_BYTES)?;
    query_template(connection, template_id)?
        .ok_or_else(|| AgentTemplateError::TemplateNotFound(template_id.to_string()))
}

pub fn list_project_templates(
    connection: &Connection,
    project_id: &str,
    include_disabled: bool,
) -> Result<Vec<AgentTemplateRecord>, AgentTemplateError> {
    validate_project_id(project_id)?;
    ensure_project_exists(connection, project_id)?;
    let sql = if include_disabled {
        "SELECT template.template_id, template.schema_version, template.machine_key,
                template.name, template.description, template.instructions,
                template.model_config_id, template.enabled, template.revision,
                template.created_at, template.updated_at
         FROM project_agent_template_bindings AS binding
         INNER JOIN agent_templates AS template ON template.template_id = binding.template_id
         WHERE binding.project_id = ?1
         ORDER BY template.created_at ASC, template.template_id ASC"
    } else {
        "SELECT template.template_id, template.schema_version, template.machine_key,
                template.name, template.description, template.instructions,
                template.model_config_id, template.enabled, template.revision,
                template.created_at, template.updated_at
         FROM project_agent_template_bindings AS binding
         INNER JOIN agent_templates AS template ON template.template_id = binding.template_id
         WHERE binding.project_id = ?1 AND template.enabled = 1
         ORDER BY template.created_at ASC, template.template_id ASC"
    };
    let mut statement = connection.prepare(sql).map_err(storage_failure)?;
    let rows = statement
        .query_map([project_id], read_stored_row)
        .map_err(storage_failure)?;
    let mut stored_rows = Vec::new();
    for row in rows {
        stored_rows.push(row.map_err(read_failure)?);
    }
    drop(statement);
    stored_rows
        .into_iter()
        .map(|row| decode_row(connection, row))
        .collect()
}

pub fn get_project_template_by_machine_key(
    connection: &Connection,
    project_id: &str,
    machine_key: &str,
) -> Result<AgentTemplateRecord, AgentTemplateError> {
    validate_project_id(project_id)?;
    validate_machine_key(machine_key)?;
    ensure_project_exists(connection, project_id)?;
    query_project_template_by_machine_key(connection, project_id, machine_key)?
        .ok_or_else(|| AgentTemplateError::TemplateNotFound(machine_key.to_string()))
}

/// Compares `expected_revision` and atomically replaces only editable fields.
///
/// The input intentionally has no project, template, or machine-key replacement fields, so a
/// display-name edit can never change the stable Harness `agent_type` identity.
pub fn update_template(
    connection: &mut Connection,
    input: &UpdateAgentTemplateInput,
) -> Result<AgentTemplateRecord, AgentTemplateError> {
    validate_update_input(input)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_failure)?;
    let current = query_template(&transaction, &input.template_id)?
        .ok_or_else(|| AgentTemplateError::TemplateNotFound(input.template_id.clone()))?;
    ensure_revision(&current, input.expected_revision)?;
    ensure_unique_name(&transaction, &input.name, Some(&input.template_id))?;

    let next_revision = next_revision(current.revision)?;
    let updated_at = now_ms().max(current.updated_at.saturating_add(1));
    let changed = transaction
        .execute(
            "UPDATE agent_templates
             SET name = ?1,
                 description = ?2,
                 instructions = ?3,
                 model_config_id = ?4,
                 revision = ?5,
                 updated_at = ?6
             WHERE template_id = ?7 AND revision = ?8",
            params![
                &input.name,
                &input.description,
                &input.instructions,
                &input.model_config_id,
                revision_to_sql(next_revision)?,
                updated_at,
                &input.template_id,
                revision_to_sql(input.expected_revision)?,
            ],
        )
        .map_err(storage_failure)?;
    if changed != 1 {
        return Err(corrupt(
            "template compare-and-set updated an unexpected row count",
        ));
    }
    let stored = query_template(&transaction, &input.template_id)?
        .ok_or_else(|| corrupt("updated template could not be read back"))?;
    transaction.commit().map_err(storage_failure)?;
    Ok(stored)
}

pub fn set_template_enabled(
    connection: &mut Connection,
    template_id: &str,
    expected_revision: u64,
    enabled: bool,
) -> Result<AgentTemplateRecord, AgentTemplateError> {
    validate_opaque_id("template_id", template_id, MAX_TEMPLATE_ID_BYTES)?;
    validate_expected_revision(expected_revision)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_failure)?;
    let current = query_template(&transaction, template_id)?
        .ok_or_else(|| AgentTemplateError::TemplateNotFound(template_id.to_string()))?;
    ensure_revision(&current, expected_revision)?;

    let next_revision = next_revision(current.revision)?;
    let updated_at = now_ms().max(current.updated_at.saturating_add(1));
    let changed = transaction
        .execute(
            "UPDATE agent_templates
             SET enabled = ?1, revision = ?2, updated_at = ?3
             WHERE template_id = ?4 AND revision = ?5",
            params![
                enabled,
                revision_to_sql(next_revision)?,
                updated_at,
                template_id,
                revision_to_sql(expected_revision)?,
            ],
        )
        .map_err(storage_failure)?;
    if changed != 1 {
        return Err(corrupt(
            "template enable compare-and-set updated an unexpected row count",
        ));
    }
    let stored = query_template(&transaction, template_id)?
        .ok_or_else(|| corrupt("enabled template could not be read back"))?;
    transaction.commit().map_err(storage_failure)?;
    Ok(stored)
}

pub fn delete_template(
    connection: &mut Connection,
    template_id: &str,
    expected_revision: u64,
) -> Result<AgentTemplateRecord, AgentTemplateError> {
    validate_opaque_id("template_id", template_id, MAX_TEMPLATE_ID_BYTES)?;
    validate_expected_revision(expected_revision)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_failure)?;
    let current = query_template(&transaction, template_id)?
        .ok_or_else(|| AgentTemplateError::TemplateNotFound(template_id.to_string()))?;
    ensure_revision(&current, expected_revision)?;
    let snapshotted_agent_count = transaction
        .query_row(
            "SELECT COUNT(*)
             FROM agent_nodes
             WHERE template_id_snapshot = ?1",
            [template_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_failure)?;
    let snapshotted_agent_count = u64::try_from(snapshotted_agent_count)
        .map_err(|_| corrupt("template Agent reference count is invalid"))?;
    if snapshotted_agent_count > 0 {
        return Err(AgentTemplateError::TemplateInUse {
            template_id: template_id.to_string(),
            agent_count: snapshotted_agent_count,
        });
    }
    let changed = transaction
        .execute(
            "DELETE FROM agent_templates
             WHERE template_id = ?1 AND revision = ?2",
            params![template_id, revision_to_sql(expected_revision)?],
        )
        .map_err(storage_failure)?;
    if changed != 1 {
        return Err(corrupt(
            "template compare-and-set deleted an unexpected row count",
        ));
    }
    transaction.commit().map_err(storage_failure)?;
    Ok(current)
}

pub fn set_template_project_assignment(
    connection: &mut Connection,
    project_id: &str,
    template_id: &str,
    assigned: bool,
) -> Result<AgentTemplateRecord, AgentTemplateError> {
    validate_project_id(project_id)?;
    validate_opaque_id("template_id", template_id, MAX_TEMPLATE_ID_BYTES)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_failure)?;
    ensure_project_exists(&transaction, project_id)?;
    query_template(&transaction, template_id)?
        .ok_or_else(|| AgentTemplateError::TemplateNotFound(template_id.to_string()))?;

    if assigned {
        let already_assigned = binding_exists(&transaction, project_id, template_id)?;
        if !already_assigned {
            let assigned_count = project_template_count(&transaction, project_id)?;
            if assigned_count >= u64::from(MAX_PROJECT_TEMPLATES) {
                return Err(AgentTemplateError::ProjectTemplateLimit {
                    project_id: project_id.to_string(),
                    limit: MAX_PROJECT_TEMPLATES,
                });
            }
            transaction
                .execute(
                    "INSERT INTO project_agent_template_bindings (
                         project_id, template_id, created_at
                     ) VALUES (?1, ?2, ?3)",
                    params![project_id, template_id, now_ms()],
                )
                .map_err(storage_failure)?;
        }
    } else {
        transaction
            .execute(
                "DELETE FROM project_agent_template_bindings
                 WHERE project_id = ?1 AND template_id = ?2",
                params![project_id, template_id],
            )
            .map_err(storage_failure)?;
    }

    let stored = query_template(&transaction, template_id)?
        .ok_or_else(|| corrupt("assigned template could not be read back"))?;
    transaction.commit().map_err(storage_failure)?;
    Ok(stored)
}

fn query_template(
    connection: &Connection,
    template_id: &str,
) -> Result<Option<AgentTemplateRecord>, AgentTemplateError> {
    let row = connection
        .query_row(
            "SELECT template_id, schema_version, machine_key, name, description,
                    instructions, model_config_id, enabled, revision, created_at, updated_at
             FROM agent_templates
             WHERE template_id = ?1",
            [template_id],
            read_stored_row,
        )
        .optional()
        .map_err(read_failure)?;
    row.map(|row| decode_row(connection, row)).transpose()
}

fn query_project_template_by_machine_key(
    connection: &Connection,
    project_id: &str,
    machine_key: &str,
) -> Result<Option<AgentTemplateRecord>, AgentTemplateError> {
    let row = connection
        .query_row(
            "SELECT template.template_id, template.schema_version, template.machine_key,
                    template.name, template.description, template.instructions,
                    template.model_config_id, template.enabled, template.revision,
                    template.created_at, template.updated_at
             FROM project_agent_template_bindings AS binding
             INNER JOIN agent_templates AS template ON template.template_id = binding.template_id
             WHERE binding.project_id = ?1 AND template.machine_key = ?2",
            params![project_id, machine_key],
            read_stored_row,
        )
        .optional()
        .map_err(read_failure)?;
    row.map(|row| decode_row(connection, row)).transpose()
}

fn read_stored_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredTemplateRow> {
    Ok(StoredTemplateRow {
        template_id: row.get(0)?,
        schema_version: row.get(1)?,
        machine_key: row.get(2)?,
        name: row.get(3)?,
        description: row.get(4)?,
        instructions: row.get(5)?,
        model_config_id: row.get(6)?,
        enabled: row.get(7)?,
        revision: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn decode_row(
    connection: &Connection,
    row: StoredTemplateRow,
) -> Result<AgentTemplateRecord, AgentTemplateError> {
    let schema_version = u32::try_from(row.schema_version)
        .map_err(|_| corrupt("schema version is outside the supported integer range"))?;
    if schema_version != AGENT_GRAPH_SCHEMA_VERSION {
        return Err(corrupt("schema version is unsupported"));
    }
    let enabled = match row.enabled {
        0 => false,
        1 => true,
        _ => return Err(corrupt("enabled is not boolean")),
    };
    let revision = u64::try_from(row.revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| corrupt("revision is invalid"))?;
    if row.created_at < 0 || row.updated_at < row.created_at {
        return Err(corrupt("timestamps are invalid"));
    }
    validate_opaque_id("template_id", &row.template_id, MAX_TEMPLATE_ID_BYTES)
        .map_err(persisted_validation_failure)?;
    validate_machine_key(&row.machine_key).map_err(persisted_validation_failure)?;
    validate_editable_fields(
        &row.name,
        &row.description,
        &row.instructions,
        &row.model_config_id,
    )
    .map_err(persisted_validation_failure)?;

    let project_ids = query_project_ids(connection, &row.template_id)?;
    Ok(AgentTemplateRecord {
        template_id: row.template_id,
        project_ids,
        machine_key: row.machine_key,
        name: row.name,
        description: row.description,
        instructions: row.instructions,
        model_config_id: row.model_config_id,
        enabled,
        revision,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn validate_create_input(input: &CreateAgentTemplateInput) -> Result<(), AgentTemplateError> {
    validate_opaque_id("template_id", &input.template_id, MAX_TEMPLATE_ID_BYTES)?;
    validate_machine_key(&input.machine_key)?;
    validate_editable_fields(
        &input.name,
        &input.description,
        &input.instructions,
        &input.model_config_id,
    )
}

fn validate_update_input(input: &UpdateAgentTemplateInput) -> Result<(), AgentTemplateError> {
    validate_opaque_id("template_id", &input.template_id, MAX_TEMPLATE_ID_BYTES)?;
    validate_expected_revision(input.expected_revision)?;
    validate_editable_fields(
        &input.name,
        &input.description,
        &input.instructions,
        &input.model_config_id,
    )
}

fn validate_editable_fields(
    name: &str,
    description: &str,
    instructions: &str,
    model_config_id: &str,
) -> Result<(), AgentTemplateError> {
    validate_trimmed_text("name", name, 1, MAX_NAME_BYTES)?;
    if name.chars().any(char::is_control) {
        return invalid("name", "contains a control character");
    }
    validate_bounded_text("description", description, 0, MAX_DESCRIPTION_BYTES)?;
    validate_trimmed_text("instructions", instructions, 1, MAX_INSTRUCTIONS_BYTES)?;
    validate_trimmed_text(
        "model_config_id",
        model_config_id,
        1,
        MAX_MODEL_CONFIG_ID_BYTES,
    )
}

fn validate_machine_key(machine_key: &str) -> Result<(), AgentTemplateError> {
    if machine_key.is_empty() || machine_key.len() > MAX_MACHINE_KEY_BYTES {
        return invalid("machine_key", "must contain between 1 and 64 ASCII bytes");
    }
    let mut bytes = machine_key.bytes();
    if !bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        || !machine_key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return invalid(
            "machine_key",
            "must start with a lowercase letter and contain only a-z, 0-9, _ or -",
        );
    }
    Ok(())
}

fn validate_project_id(project_id: &str) -> Result<(), AgentTemplateError> {
    validate_opaque_id("project_id", project_id, 1024)
}

fn validate_opaque_id(
    field: &'static str,
    value: &str,
    maximum_bytes: usize,
) -> Result<(), AgentTemplateError> {
    if value.is_empty() || value.len() > maximum_bytes || value.contains('\0') {
        return invalid(
            field,
            format!("must contain between 1 and {maximum_bytes} bytes and no NUL"),
        );
    }
    Ok(())
}

fn validate_trimmed_text(
    field: &'static str,
    value: &str,
    minimum_bytes: usize,
    maximum_bytes: usize,
) -> Result<(), AgentTemplateError> {
    validate_bounded_text(field, value, minimum_bytes, maximum_bytes)?;
    if value.trim() != value {
        return invalid(field, "must not contain leading or trailing whitespace");
    }
    Ok(())
}

fn validate_bounded_text(
    field: &'static str,
    value: &str,
    minimum_bytes: usize,
    maximum_bytes: usize,
) -> Result<(), AgentTemplateError> {
    if value.len() < minimum_bytes || value.len() > maximum_bytes || value.contains('\0') {
        return invalid(
            field,
            format!("must contain between {minimum_bytes} and {maximum_bytes} bytes and no NUL"),
        );
    }
    Ok(())
}

fn validate_expected_revision(expected_revision: u64) -> Result<(), AgentTemplateError> {
    if expected_revision == 0 || expected_revision > MAX_SQLITE_REVISION {
        return invalid("expected_revision", "must be a positive SQLite integer");
    }
    Ok(())
}

fn ensure_project_exists(
    connection: &Connection,
    project_id: &str,
) -> Result<(), AgentTemplateError> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            [project_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_failure)?;
    if exists {
        Ok(())
    } else {
        Err(AgentTemplateError::ProjectNotFound(project_id.to_string()))
    }
}

fn template_id_exists(
    connection: &Connection,
    template_id: &str,
) -> Result<bool, AgentTemplateError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_templates WHERE template_id = ?1)",
            [template_id],
            |row| row.get(0),
        )
        .map_err(storage_failure)
}

fn query_project_ids(
    connection: &Connection,
    template_id: &str,
) -> Result<Vec<String>, AgentTemplateError> {
    let mut statement = connection
        .prepare(
            "SELECT project_id
             FROM project_agent_template_bindings
             WHERE template_id = ?1
             ORDER BY project_id ASC",
        )
        .map_err(storage_failure)?;
    let rows = statement
        .query_map([template_id], |row| row.get::<_, String>(0))
        .map_err(storage_failure)?;
    let mut project_ids = Vec::new();
    for row in rows {
        let project_id = row.map_err(read_failure)?;
        validate_project_id(&project_id).map_err(persisted_validation_failure)?;
        project_ids.push(project_id);
    }
    Ok(project_ids)
}

fn binding_exists(
    connection: &Connection,
    project_id: &str,
    template_id: &str,
) -> Result<bool, AgentTemplateError> {
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM project_agent_template_bindings
                 WHERE project_id = ?1 AND template_id = ?2
             )",
            params![project_id, template_id],
            |row| row.get(0),
        )
        .map_err(storage_failure)
}

fn project_template_count(
    connection: &Connection,
    project_id: &str,
) -> Result<u64, AgentTemplateError> {
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM project_agent_template_bindings WHERE project_id = ?1",
            [project_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_failure)?;
    u64::try_from(count).map_err(|_| corrupt("project Agent template count is invalid"))
}

fn ensure_unique_machine_key(
    connection: &Connection,
    machine_key: &str,
    excluding_template_id: Option<&str>,
) -> Result<(), AgentTemplateError> {
    let conflict = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM agent_templates
                WHERE machine_key = ?1
                  AND (?2 IS NULL OR template_id != ?2)
             )",
            params![machine_key, excluding_template_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_failure)?;
    if conflict {
        Err(AgentTemplateError::MachineKeyConflict(
            machine_key.to_string(),
        ))
    } else {
        Ok(())
    }
}

fn ensure_unique_name(
    connection: &Connection,
    name: &str,
    excluding_template_id: Option<&str>,
) -> Result<(), AgentTemplateError> {
    let conflict = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM agent_templates
                WHERE name = ?1
                  AND (?2 IS NULL OR template_id != ?2)
             )",
            params![name, excluding_template_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_failure)?;
    if conflict {
        Err(AgentTemplateError::NameConflict(name.to_string()))
    } else {
        Ok(())
    }
}

fn ensure_revision(current: &AgentTemplateRecord, expected: u64) -> Result<(), AgentTemplateError> {
    if current.revision == expected {
        Ok(())
    } else {
        Err(AgentTemplateError::RevisionConflict {
            expected,
            current: current.revision,
        })
    }
}

fn next_revision(current: u64) -> Result<u64, AgentTemplateError> {
    current
        .checked_add(1)
        .filter(|revision| *revision <= MAX_SQLITE_REVISION)
        .ok_or(AgentTemplateError::RevisionExhausted)
}

fn revision_to_sql(revision: u64) -> Result<i64, AgentTemplateError> {
    i64::try_from(revision).map_err(|_| AgentTemplateError::RevisionExhausted)
}

fn invalid<T>(field: &'static str, reason: impl Into<String>) -> Result<T, AgentTemplateError> {
    Err(AgentTemplateError::InvalidInput {
        field,
        reason: reason.into(),
    })
}

fn corrupt(reason: impl Into<String>) -> AgentTemplateError {
    AgentTemplateError::CorruptRecord(reason.into())
}

fn persisted_validation_failure(error: AgentTemplateError) -> AgentTemplateError {
    corrupt(error.to_string())
}

fn storage_failure(_: rusqlite::Error) -> AgentTemplateError {
    AgentTemplateError::StorageUnavailable("database operation failed".to_string())
}

fn read_failure(error: rusqlite::Error) -> AgentTemplateError {
    match error {
        rusqlite::Error::InvalidColumnType(..)
        | rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..) => {
            corrupt("persisted template column has an invalid type or range")
        }
        error => storage_failure(error),
    }
}
