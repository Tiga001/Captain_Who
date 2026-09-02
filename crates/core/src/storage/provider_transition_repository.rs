use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::error::Error;
use std::fmt::{Display, Formatter};

pub const PROVIDER_TRANSITION_TERMINAL_RECORD_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTransitionTerminalRecord {
    pub schema_version: u32,
    pub operation_id: String,
    pub conversation_id: String,
    pub target_model_id: String,
    /// Immutable UI labels only. They are never used for routing or model mutation.
    pub source_model_display_name: Option<String>,
    pub target_model_display_name: Option<String>,
    pub started_at: i64,
    pub completed_at: i64,
    pub conversation_updated_at: i64,
}

impl ProviderTransitionTerminalRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        operation_id: impl Into<String>,
        conversation_id: impl Into<String>,
        target_model_id: impl Into<String>,
        source_model_display_name: Option<String>,
        target_model_display_name: Option<String>,
        started_at: i64,
        completed_at: i64,
        conversation_updated_at: i64,
    ) -> Result<Self, ProviderTransitionRepositoryError> {
        let record = Self {
            schema_version: PROVIDER_TRANSITION_TERMINAL_RECORD_SCHEMA_VERSION,
            operation_id: operation_id.into(),
            conversation_id: conversation_id.into(),
            target_model_id: target_model_id.into(),
            source_model_display_name,
            target_model_display_name,
            started_at,
            completed_at,
            conversation_updated_at,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), ProviderTransitionRepositoryError> {
        if self.schema_version != PROVIDER_TRANSITION_TERMINAL_RECORD_SCHEMA_VERSION {
            return Err(ProviderTransitionRepositoryError::Invalid(
                "Provider transition terminal record schema version is unsupported.".to_string(),
            ));
        }
        validate_identity("operation_id", &self.operation_id, 1024)?;
        if !self.operation_id.starts_with("provider-transition-")
            || self.operation_id.len() <= "provider-transition-".len()
        {
            return Err(ProviderTransitionRepositoryError::Invalid(
                "Provider transition operation identity has an invalid prefix.".to_string(),
            ));
        }
        validate_identity("conversation_id", &self.conversation_id, 512)?;
        validate_identity("target_model_id", &self.target_model_id, 512)?;
        match (
            self.source_model_display_name.as_deref(),
            self.target_model_display_name.as_deref(),
        ) {
            (Some(source), Some(target)) => {
                validate_identity("source_model_display_name", source, 512)?;
                validate_identity("target_model_display_name", target, 512)?;
            }
            (None, None) => {}
            _ => {
                return Err(ProviderTransitionRepositoryError::Invalid(
                    "Provider transition display names must either both be present or both be absent."
                        .to_string(),
                ));
            }
        }
        if self.started_at < 0
            || self.completed_at < self.started_at
            || self.conversation_updated_at < self.started_at
        {
            return Err(ProviderTransitionRepositoryError::Invalid(
                "Provider transition terminal timestamps are invalid.".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderTransitionCompatibleCommitOutcome {
    Committed(ProviderTransitionTerminalRecord),
    AlreadyCommitted(ProviderTransitionTerminalRecord),
    Stale,
}

#[derive(Debug)]
pub enum ProviderTransitionRepositoryError {
    Database(rusqlite::Error),
    Invalid(String),
}

impl Display for ProviderTransitionRepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "本地数据库操作失败：{error}"),
            Self::Invalid(message) => write!(formatter, "Provider transition 记录无效：{message}"),
        }
    }
}

impl Error for ProviderTransitionRepositoryError {}

impl From<rusqlite::Error> for ProviderTransitionRepositoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn commit_compatible_transition(
    connection: &mut Connection,
    record: &ProviderTransitionTerminalRecord,
    expected_current_model_id: Option<&str>,
    expected_conversation_updated_at: i64,
    expected_conversation_revision: i64,
    expected_target_provider_protocol_revision: &str,
) -> Result<ProviderTransitionCompatibleCommitOutcome, ProviderTransitionRepositoryError> {
    record.validate()?;
    if let Some(existing) = get_terminal_record(connection, &record.operation_id)? {
        if existing.conversation_id != record.conversation_id
            || existing.target_model_id != record.target_model_id
        {
            return Err(ProviderTransitionRepositoryError::Invalid(
                "Provider transition operation identity is already bound to another target."
                    .to_string(),
            ));
        }
        return Ok(ProviderTransitionCompatibleCommitOutcome::AlreadyCommitted(
            existing,
        ));
    }

    let transaction = connection.transaction()?;
    if !provider_transition_compare_and_set_is_current(
        &transaction,
        &record.conversation_id,
        expected_current_model_id,
        expected_conversation_updated_at,
        expected_conversation_revision,
        &record.target_model_id,
        expected_target_provider_protocol_revision,
    )? {
        return Ok(ProviderTransitionCompatibleCommitOutcome::Stale);
    }
    let mut committed = record.clone();
    committed.conversation_updated_at = provider_transition_updated_at(
        &transaction,
        &record.conversation_id,
        record.conversation_updated_at,
    )?;
    committed.validate()?;
    apply_model_selection(&transaction, &committed)?;
    insert_terminal_record(&transaction, &committed)?;
    transaction.commit()?;
    Ok(ProviderTransitionCompatibleCommitOutcome::Committed(
        committed,
    ))
}

fn provider_transition_updated_at(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    minimum_updated_at: i64,
) -> Result<i64, ProviderTransitionRepositoryError> {
    let draft_updated_at = transaction
        .query_row(
            "SELECT updated_at FROM composer_drafts WHERE scope_id = ?1",
            [conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(draft_updated_at
        .map(|updated_at| minimum_updated_at.max(updated_at.saturating_add(1)))
        .unwrap_or(minimum_updated_at))
}

pub fn get_terminal_record(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<ProviderTransitionTerminalRecord>, ProviderTransitionRepositoryError> {
    connection
        .query_row(
            "SELECT schema_version, operation_id, conversation_id, target_model_id,
                    source_model_display_name, target_model_display_name,
                    started_at, completed_at, conversation_updated_at
             FROM provider_transition_terminal_records
             WHERE operation_id = ?1",
            [operation_id],
            decode_terminal_record,
        )
        .optional()
        .map_err(Into::into)
        .and_then(|record| record.map(validate_decoded_record).transpose())
}

pub fn list_terminal_records(
    connection: &Connection,
    conversation_id: &str,
    limit: usize,
) -> Result<Vec<ProviderTransitionTerminalRecord>, ProviderTransitionRepositoryError> {
    validate_identity("conversation_id", conversation_id, 512)?;
    let bounded_limit = i64::try_from(limit.clamp(1, 100)).unwrap_or(100);
    let mut statement = connection.prepare(
        "SELECT schema_version, operation_id, conversation_id, target_model_id,
                source_model_display_name, target_model_display_name,
                started_at, completed_at, conversation_updated_at
         FROM provider_transition_terminal_records
         WHERE conversation_id = ?1
         ORDER BY started_at DESC, operation_id DESC
         LIMIT ?2",
    )?;
    let records = statement
        .query_map(
            params![conversation_id, bounded_limit],
            decode_terminal_record,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    records.into_iter().map(validate_decoded_record).collect()
}

fn provider_transition_compare_and_set_is_current(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    expected_current_model_id: Option<&str>,
    expected_conversation_updated_at: i64,
    expected_conversation_revision: i64,
    target_model_id: &str,
    expected_target_provider_protocol_revision: &str,
) -> Result<bool, ProviderTransitionRepositoryError> {
    let conversation = transaction
        .query_row(
            "SELECT model_id, updated_at, revision FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((current_model_id, current_updated_at, current_revision)) = conversation else {
        return Ok(false);
    };
    if current_model_id.as_deref() != expected_current_model_id
        || current_updated_at != expected_conversation_updated_at
        || current_revision != expected_conversation_revision
        || !graph_allows_provider_transition_commit(transaction, conversation_id)?
    {
        return Ok(false);
    }
    let target_revision = transaction
        .query_row(
            "SELECT provider_protocol_revision FROM models WHERE id = ?1 AND enabled = 1",
            [target_model_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(target_revision.as_deref() == Some(expected_target_provider_protocol_revision))
}

/// Provider-transition commits are user/root mutations. Legacy Conversations may remain unbound,
/// but once Graph identity exists only an active root with no active Turn may be changed. Keeping
/// this check in the final SQLite transaction closes lifecycle and cross-Host admission races.
pub(crate) fn graph_allows_provider_transition_commit(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<bool> {
    let binding = connection
        .query_row(
            "SELECT parent_agent_id, lifecycle
             FROM agent_nodes WHERE conversation_id = ?1",
            [conversation_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if binding
        .as_ref()
        .is_some_and(|(parent_agent_id, lifecycle)| {
            parent_agent_id.is_some() || lifecycle != "active"
        })
    {
        return Ok(false);
    }
    let active_turn = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM conversation_turn_traces
             WHERE conversation_id = ?1 AND terminal_status = 'in_progress'
         )",
        [conversation_id],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(!active_turn)
}

fn apply_model_selection(
    transaction: &Transaction<'_>,
    record: &ProviderTransitionTerminalRecord,
) -> Result<(), ProviderTransitionRepositoryError> {
    let changed = transaction.execute(
        "UPDATE conversations SET model_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![
            &record.target_model_id,
            record.conversation_updated_at,
            &record.conversation_id
        ],
    )?;
    if changed != 1 {
        return Err(ProviderTransitionRepositoryError::Invalid(
            "Provider transition conversation disappeared during commit.".to_string(),
        ));
    }
    transaction.execute(
        "UPDATE composer_drafts SET model_id = ?1, updated_at = ?2 WHERE scope_id = ?3",
        params![
            &record.target_model_id,
            record.conversation_updated_at,
            &record.conversation_id
        ],
    )?;
    Ok(())
}

fn insert_terminal_record(
    transaction: &Transaction<'_>,
    record: &ProviderTransitionTerminalRecord,
) -> Result<(), ProviderTransitionRepositoryError> {
    transaction.execute(
        "INSERT INTO provider_transition_terminal_records (
            schema_version, operation_id, conversation_id, target_model_id,
            source_model_display_name, target_model_display_name,
            started_at, completed_at, conversation_updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            record.schema_version,
            &record.operation_id,
            &record.conversation_id,
            &record.target_model_id,
            &record.source_model_display_name,
            &record.target_model_display_name,
            record.started_at,
            record.completed_at,
            record.conversation_updated_at,
        ],
    )?;
    Ok(())
}

fn decode_terminal_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ProviderTransitionTerminalRecord> {
    Ok(ProviderTransitionTerminalRecord {
        schema_version: row.get(0)?,
        operation_id: row.get(1)?,
        conversation_id: row.get(2)?,
        target_model_id: row.get(3)?,
        source_model_display_name: row.get(4)?,
        target_model_display_name: row.get(5)?,
        started_at: row.get(6)?,
        completed_at: row.get(7)?,
        conversation_updated_at: row.get(8)?,
    })
}

fn validate_decoded_record(
    record: ProviderTransitionTerminalRecord,
) -> Result<ProviderTransitionTerminalRecord, ProviderTransitionRepositoryError> {
    record.validate()?;
    Ok(record)
}

fn validate_identity(
    field: &str,
    value: &str,
    maximum_bytes: usize,
) -> Result<(), ProviderTransitionRepositoryError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > maximum_bytes || trimmed != value {
        return Err(ProviderTransitionRepositoryError::Invalid(format!(
            "{field} must be a trimmed non-empty string no longer than {maximum_bytes} bytes."
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    fn setup() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let provider_profile =
            serde_json::to_string(&crate::ProviderProfileConfig::generic_for_dialect(
                crate::ProviderProtocolDialect::OpenAiChatCompletions,
            ))
            .unwrap();
        connection
            .execute(
                "INSERT INTO models (
                    id, provider_model_id, display_name, normalized_display_name,
                    supports_image, provider_connection_revision,
                    provider_protocol_revision, provider_profile_config_json,
                    input_price, output_price, enabled, position, created_at, updated_at
                 ) VALUES ('target-model', 'target-model', 'Target', 'target', 0,
                           'provider-connection-v1:target-connection',
                           'provider-protocol-v1:target-revision', ?1,
                           '0', '0', 1, 0, 1, 1)",
                [&provider_profile],
            )
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-1', NULL, 'source-model', 'Test', 1, 7,
                           NULL, NULL, NULL);
                 INSERT INTO composer_drafts (
                    scope_id, message, permission_mode, permission_mode_version,
                    model_id, project_id, attachments_json, skills_json,
                    queued_messages_json, updated_at
                 ) VALUES ('conversation-1', 'keep me', 'ask', 1, 'source-model', NULL,
                           '[]', '[]', '[]', 7);",
            )
            .unwrap();
        connection
    }

    fn record(operation_id: &str) -> ProviderTransitionTerminalRecord {
        ProviderTransitionTerminalRecord::new(
            operation_id,
            "conversation-1",
            "target-model",
            Some("Source".to_string()),
            Some("Target".to_string()),
            10,
            12,
            12,
        )
        .unwrap()
    }

    #[test]
    fn compatible_commit_atomically_switches_model_draft_and_terminal_record() {
        let mut connection = setup();
        connection
            .execute(
                "UPDATE composer_drafts SET updated_at = 100 WHERE scope_id = 'conversation-1'",
                [],
            )
            .unwrap();
        let terminal = record("provider-transition-compatible-1");
        let mut committed_terminal = terminal.clone();
        committed_terminal.conversation_updated_at = 101;
        let outcome = commit_compatible_transition(
            &mut connection,
            &terminal,
            Some("source-model"),
            7,
            0,
            "provider-protocol-v1:target-revision",
        )
        .unwrap();
        assert_eq!(
            outcome,
            ProviderTransitionCompatibleCommitOutcome::Committed(committed_terminal.clone())
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id, updated_at FROM conversations WHERE id = 'conversation-1'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("target-model".to_string(), 101)
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id, message FROM composer_drafts WHERE scope_id = 'conversation-1'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .unwrap(),
            ("target-model".to_string(), "keep me".to_string())
        );
        assert_eq!(
            get_terminal_record(&connection, &terminal.operation_id).unwrap(),
            Some(committed_terminal.clone())
        );

        let duplicate = commit_compatible_transition(
            &mut connection,
            &terminal,
            Some("source-model"),
            7,
            0,
            "provider-protocol-v1:target-revision",
        )
        .unwrap();
        assert_eq!(
            duplicate,
            ProviderTransitionCompatibleCommitOutcome::AlreadyCommitted(committed_terminal)
        );
    }

    #[test]
    fn stale_compatible_commit_writes_no_terminal_or_model_half_state() {
        let mut connection = setup();
        let terminal = record("provider-transition-compatible-stale");
        let outcome = commit_compatible_transition(
            &mut connection,
            &terminal,
            Some("source-model"),
            7,
            0,
            "wrong-revision",
        )
        .unwrap();
        assert_eq!(outcome, ProviderTransitionCompatibleCommitOutcome::Stale);
        assert!(get_terminal_record(&connection, &terminal.operation_id)
            .unwrap()
            .is_none());
        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id FROM conversations WHERE id = 'conversation-1'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "source-model"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id FROM composer_drafts WHERE scope_id = 'conversation-1'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "source-model"
        );
    }

    #[test]
    fn compatible_commit_rechecks_revision_and_active_root_inside_its_transaction() {
        let mut connection = setup();
        let stale_revision = record("provider-transition-compatible-stale-revision");
        let outcome = commit_compatible_transition(
            &mut connection,
            &stale_revision,
            Some("source-model"),
            7,
            1,
            "provider-protocol-v1:target-revision",
        )
        .unwrap();
        assert_eq!(outcome, ProviderTransitionCompatibleCommitOutcome::Stale);
        assert!(
            get_terminal_record(&connection, &stale_revision.operation_id)
                .unwrap()
                .is_none()
        );

        crate::storage::agent_graph_repository::ensure_root_agent(
            &mut connection,
            &crate::EnsureRootAgentInput {
                agent_id: "agent-provider-transition-root".to_string(),
                conversation_id: "conversation-1".to_string(),
                creation_request_id: "ensure-provider-transition-root".to_string(),
                task_name: "Root".to_string(),
            },
            2,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE agent_nodes
                 SET lifecycle = 'disabled', revision = revision + 1, updated_at = 3
                 WHERE agent_id = 'agent-provider-transition-root'",
                [],
            )
            .unwrap();
        let inactive_root = record("provider-transition-compatible-inactive-root");
        let outcome = commit_compatible_transition(
            &mut connection,
            &inactive_root,
            Some("source-model"),
            7,
            0,
            "provider-protocol-v1:target-revision",
        )
        .unwrap();
        assert_eq!(outcome, ProviderTransitionCompatibleCommitOutcome::Stale);
        assert!(
            get_terminal_record(&connection, &inactive_root.operation_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id FROM conversations WHERE id = 'conversation-1'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "source-model"
        );
    }

    #[test]
    fn terminal_records_list_newest_first_and_cascade_with_conversation() {
        let mut connection = setup();
        let first = record("provider-transition-compatible-first");
        commit_compatible_transition(
            &mut connection,
            &first,
            Some("source-model"),
            7,
            0,
            "provider-protocol-v1:target-revision",
        )
        .unwrap();
        let rows = list_terminal_records(&connection, "conversation-1", 50).unwrap();
        assert_eq!(rows, vec![first]);
        connection
            .execute("DELETE FROM conversations WHERE id = 'conversation-1'", [])
            .unwrap();
        assert!(list_terminal_records(&connection, "conversation-1", 50)
            .unwrap()
            .is_empty());
    }
}
