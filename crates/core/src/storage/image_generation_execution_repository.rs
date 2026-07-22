//! Durable, replay-safe journal for image-generation executions.
//!
//! An execution claim is permanent, not a lease. A process restart may reconcile local Artifact
//! publication, but must never replay a provider request for an existing execution identity.

use crate::storage::now_ms;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub const IMAGE_GENERATION_EXECUTION_SCHEMA_VERSION: u32 = 1;
pub const IMAGE_GENERATION_ARTIFACT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredImageGenerationExecutionStatus {
    Executing,
    Publishing,
    Succeeded,
    Failed,
    Cancelled,
    OutcomeIndeterminate,
    CommitIndeterminate,
}

impl StoredImageGenerationExecutionStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Executing => "executing",
            Self::Publishing => "publishing",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::OutcomeIndeterminate => "outcome_indeterminate",
            Self::CommitIndeterminate => "commit_indeterminate",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "executing" => Ok(Self::Executing),
            "publishing" => Ok(Self::Publishing),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "outcome_indeterminate" => Ok(Self::OutcomeIndeterminate),
            "commit_indeterminate" => Ok(Self::CommitIndeterminate),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Executing | Self::Publishing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredImageGenerationArtifactState {
    Candidate,
    Published,
    Discarded,
    Indeterminate,
}

impl StoredImageGenerationArtifactState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Published => "published",
            Self::Discarded => "discarded",
            Self::Indeterminate => "indeterminate",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "candidate" => Ok(Self::Candidate),
            "published" => Ok(Self::Published),
            "discarded" => Ok(Self::Discarded),
            "indeterminate" => Ok(Self::Indeterminate),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationExecutionIdentityRecord {
    pub execution_id: String,
    pub request_fingerprint: String,
    pub safe_request_json: String,
    pub profile_id: String,
    pub adapter_id: String,
    pub profile_revision: u64,
    pub model_id: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationArtifactJournalRecord {
    pub ordinal: u32,
    pub artifact_id: String,
    pub state: StoredImageGenerationArtifactState,
    pub storage_relative_path: String,
    pub format: String,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at: i64,
    pub published_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationExecutionJournalRecord {
    pub identity: ImageGenerationExecutionIdentityRecord,
    pub status: StoredImageGenerationExecutionStatus,
    pub remote_outcome_unknown: bool,
    pub provider_succeeded: bool,
    pub commit_may_have_succeeded: bool,
    pub provider_request_id: Option<String>,
    pub http_status: Option<u16>,
    pub terminal_result_json: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
    pub artifact: Option<ImageGenerationArtifactJournalRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageGenerationExecutionClaimOutcome {
    Claimed(ImageGenerationExecutionJournalRecord),
    Existing(ImageGenerationExecutionJournalRecord),
    IdentityConflict(ImageGenerationExecutionJournalRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageGenerationExecutionMutationOutcome {
    Updated(ImageGenerationExecutionJournalRecord),
    AlreadyCurrent(ImageGenerationExecutionJournalRecord),
    StateConflict(Option<ImageGenerationExecutionJournalRecord>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationExecutionTerminalUpdate {
    pub expected_request_fingerprint: String,
    pub expected_artifact_sha256: Option<String>,
    pub status: StoredImageGenerationExecutionStatus,
    pub remote_outcome_unknown: bool,
    pub provider_succeeded: bool,
    pub commit_may_have_succeeded: bool,
    pub provider_request_id: Option<String>,
    pub http_status: Option<u16>,
    pub terminal_result_json: String,
}

pub fn claim_image_generation_execution(
    connection: &mut Connection,
    identity: &ImageGenerationExecutionIdentityRecord,
) -> rusqlite::Result<ImageGenerationExecutionClaimOutcome> {
    let transaction = connection.transaction()?;
    if let Some(existing) = query_execution_transaction(&transaction, &identity.execution_id)? {
        let outcome = if existing.identity == *identity {
            ImageGenerationExecutionClaimOutcome::Existing(existing)
        } else {
            ImageGenerationExecutionClaimOutcome::IdentityConflict(existing)
        };
        transaction.rollback()?;
        return Ok(outcome);
    }
    let timestamp = now_ms();
    let profile_revision = i64::try_from(identity.profile_revision)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    transaction.execute(
        "INSERT INTO image_generation_executions (
            execution_id, schema_version, request_fingerprint, safe_request_json,
            profile_id, adapter_id, profile_revision, model_id, operation, status,
            remote_outcome_unknown, provider_succeeded, commit_may_have_succeeded,
            provider_request_id, http_status, terminal_result_json,
            created_at, updated_at, completed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'executing',
             0, 0, 0, NULL, NULL, NULL, ?10, ?10, NULL)",
        params![
            &identity.execution_id,
            IMAGE_GENERATION_EXECUTION_SCHEMA_VERSION,
            &identity.request_fingerprint,
            &identity.safe_request_json,
            &identity.profile_id,
            &identity.adapter_id,
            profile_revision,
            &identity.model_id,
            &identity.operation,
            timestamp,
        ],
    )?;
    let record = query_execution_transaction(&transaction, &identity.execution_id)?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    transaction.commit()?;
    Ok(ImageGenerationExecutionClaimOutcome::Claimed(record))
}

pub fn prepare_image_generation_artifact(
    connection: &mut Connection,
    execution_id: &str,
    artifact: &ImageGenerationArtifactJournalRecord,
    provider_request_id: Option<&str>,
    http_status: Option<u16>,
) -> rusqlite::Result<ImageGenerationExecutionMutationOutcome> {
    let transaction = connection.transaction()?;
    let Some(current) = query_execution_transaction(&transaction, execution_id)? else {
        transaction.rollback()?;
        return Ok(ImageGenerationExecutionMutationOutcome::StateConflict(None));
    };
    if current.status == StoredImageGenerationExecutionStatus::Publishing {
        let outcome = if current
            .artifact
            .as_ref()
            .is_some_and(|existing| same_artifact_identity(existing, artifact))
            && current.provider_request_id.as_deref() == provider_request_id
            && current.http_status == http_status
        {
            ImageGenerationExecutionMutationOutcome::AlreadyCurrent(current)
        } else {
            ImageGenerationExecutionMutationOutcome::StateConflict(Some(current))
        };
        transaction.rollback()?;
        return Ok(outcome);
    }
    if current.status != StoredImageGenerationExecutionStatus::Executing
        || current.artifact.is_some()
        || artifact.ordinal != 0
        || artifact.state != StoredImageGenerationArtifactState::Candidate
        || artifact.published_at.is_some()
    {
        transaction.rollback()?;
        return Ok(ImageGenerationExecutionMutationOutcome::StateConflict(
            Some(current),
        ));
    }
    let timestamp = now_ms().max(current.updated_at);
    transaction.execute(
        "INSERT INTO image_generation_artifacts (
            execution_id, ordinal, schema_version, artifact_id, state,
            storage_relative_path, format, media_type, width, height,
            size_bytes, sha256, created_at, published_at
         ) VALUES (?1, ?2, ?3, ?4, 'candidate', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL)",
        params![
            execution_id,
            i64::from(artifact.ordinal),
            IMAGE_GENERATION_ARTIFACT_SCHEMA_VERSION,
            &artifact.artifact_id,
            &artifact.storage_relative_path,
            &artifact.format,
            &artifact.media_type,
            i64::from(artifact.width),
            i64::from(artifact.height),
            i64::try_from(artifact.size_bytes)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
            &artifact.sha256,
            timestamp,
        ],
    )?;
    transaction.execute(
        "UPDATE image_generation_executions
         SET status = 'publishing', provider_succeeded = 1,
             provider_request_id = ?2, http_status = ?3, updated_at = ?4
         WHERE execution_id = ?1 AND status = 'executing'",
        params![
            execution_id,
            provider_request_id,
            http_status.map(i64::from),
            timestamp
        ],
    )?;
    let updated = query_execution_transaction(&transaction, execution_id)?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    transaction.commit()?;
    Ok(ImageGenerationExecutionMutationOutcome::Updated(updated))
}

fn same_artifact_identity(
    left: &ImageGenerationArtifactJournalRecord,
    right: &ImageGenerationArtifactJournalRecord,
) -> bool {
    left.ordinal == right.ordinal
        && left.artifact_id == right.artifact_id
        && left.state == right.state
        && left.storage_relative_path == right.storage_relative_path
        && left.format == right.format
        && left.media_type == right.media_type
        && left.width == right.width
        && left.height == right.height
        && left.size_bytes == right.size_bytes
        && left.sha256 == right.sha256
        && left.published_at == right.published_at
}

pub fn finalize_image_generation_execution(
    connection: &mut Connection,
    execution_id: &str,
    update: &ImageGenerationExecutionTerminalUpdate,
) -> rusqlite::Result<ImageGenerationExecutionMutationOutcome> {
    if !update.status.is_terminal() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction()?;
    let Some(current) = query_execution_transaction(&transaction, execution_id)? else {
        transaction.rollback()?;
        return Ok(ImageGenerationExecutionMutationOutcome::StateConflict(None));
    };
    if current.identity.request_fingerprint != update.expected_request_fingerprint
        || current.artifact.as_ref().map(|artifact| &artifact.sha256)
            != update.expected_artifact_sha256.as_ref()
    {
        transaction.rollback()?;
        return Ok(ImageGenerationExecutionMutationOutcome::StateConflict(
            Some(current),
        ));
    }
    let resolves_indeterminate_commit = current.status
        == StoredImageGenerationExecutionStatus::CommitIndeterminate
        && current.commit_may_have_succeeded
        && current.artifact.as_ref().is_some_and(|artifact| {
            artifact.state == StoredImageGenerationArtifactState::Indeterminate
        })
        && matches!(
            update.status,
            StoredImageGenerationExecutionStatus::Succeeded
                | StoredImageGenerationExecutionStatus::Failed
        )
        && current.provider_request_id == update.provider_request_id
        && current.http_status == update.http_status;
    if current.status.is_terminal() {
        let exact = current.status == update.status
            && current.remote_outcome_unknown == update.remote_outcome_unknown
            && current.provider_succeeded == update.provider_succeeded
            && current.commit_may_have_succeeded == update.commit_may_have_succeeded
            && current.provider_request_id == update.provider_request_id
            && current.http_status == update.http_status
            && current.terminal_result_json.as_deref() == Some(&update.terminal_result_json);
        if exact {
            transaction.rollback()?;
            return Ok(ImageGenerationExecutionMutationOutcome::AlreadyCurrent(
                current,
            ));
        }
        if !resolves_indeterminate_commit {
            transaction.rollback()?;
            return Ok(ImageGenerationExecutionMutationOutcome::StateConflict(
                Some(current),
            ));
        }
        // This is the sole terminal-state refinement: a prior atomic publish whose outcome was
        // unknown has now been verified against its frozen Artifact identity.
    }
    if update.status == StoredImageGenerationExecutionStatus::Succeeded
        && !((current.status == StoredImageGenerationExecutionStatus::Publishing
            && current.artifact.as_ref().is_some_and(|artifact| {
                artifact.state == StoredImageGenerationArtifactState::Candidate
            }))
            || resolves_indeterminate_commit)
    {
        transaction.rollback()?;
        return Ok(ImageGenerationExecutionMutationOutcome::StateConflict(
            Some(current),
        ));
    }
    let timestamp = now_ms().max(current.updated_at);
    let artifact_state = match update.status {
        StoredImageGenerationExecutionStatus::Succeeded => Some(("published", Some(timestamp))),
        StoredImageGenerationExecutionStatus::CommitIndeterminate => Some(("indeterminate", None)),
        _ if current.artifact.is_some() => Some(("discarded", None)),
        _ => None,
    };
    if let Some((state, published_at)) = artifact_state {
        let expected_artifact_state = if resolves_indeterminate_commit {
            "indeterminate"
        } else {
            "candidate"
        };
        let updated_artifacts = transaction.execute(
            "UPDATE image_generation_artifacts
             SET state = ?2, published_at = ?3
             WHERE execution_id = ?1 AND ordinal = 0 AND state = ?4",
            params![execution_id, state, published_at, expected_artifact_state],
        )?;
        if updated_artifacts != 1 {
            transaction.rollback()?;
            return Ok(ImageGenerationExecutionMutationOutcome::StateConflict(
                query_execution(connection, execution_id)?,
            ));
        }
    }
    let expected_status = current.status.as_str();
    let updated_rows = transaction.execute(
        "UPDATE image_generation_executions
         SET status = ?2,
             remote_outcome_unknown = ?3,
             provider_succeeded = ?4,
             commit_may_have_succeeded = ?5,
             provider_request_id = ?6,
             http_status = ?7,
             terminal_result_json = ?8,
             updated_at = ?9,
             completed_at = ?9
         WHERE execution_id = ?1 AND status = ?10",
        params![
            execution_id,
            update.status.as_str(),
            update.remote_outcome_unknown,
            update.provider_succeeded,
            update.commit_may_have_succeeded,
            &update.provider_request_id,
            update.http_status.map(i64::from),
            &update.terminal_result_json,
            timestamp,
            expected_status,
        ],
    )?;
    if updated_rows != 1 {
        transaction.rollback()?;
        return Ok(ImageGenerationExecutionMutationOutcome::StateConflict(
            query_execution(connection, execution_id)?,
        ));
    }
    let updated = query_execution_transaction(&transaction, execution_id)?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    transaction.commit()?;
    Ok(ImageGenerationExecutionMutationOutcome::Updated(updated))
}

pub fn inspect_image_generation_execution(
    connection: &Connection,
    execution_id: &str,
) -> rusqlite::Result<Option<ImageGenerationExecutionJournalRecord>> {
    query_execution(connection, execution_id)
}

pub fn list_interrupted_image_generation_executions(
    connection: &Connection,
) -> rusqlite::Result<Vec<ImageGenerationExecutionJournalRecord>> {
    let mut statement = connection.prepare(
        "SELECT execution_id
         FROM image_generation_executions
         WHERE status IN ('executing', 'publishing', 'commit_indeterminate')
         ORDER BY created_at ASC, execution_id ASC",
    )?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|execution_id| {
            query_execution(connection, &execution_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
        })
        .collect()
}

fn query_execution_transaction(
    transaction: &Transaction<'_>,
    execution_id: &str,
) -> rusqlite::Result<Option<ImageGenerationExecutionJournalRecord>> {
    query_execution(transaction, execution_id)
}

fn query_execution(
    connection: &Connection,
    execution_id: &str,
) -> rusqlite::Result<Option<ImageGenerationExecutionJournalRecord>> {
    let execution = connection
        .query_row(
            "SELECT
                request_fingerprint, safe_request_json, profile_id, adapter_id,
                profile_revision, model_id, operation, status,
                remote_outcome_unknown, provider_succeeded, commit_may_have_succeeded,
                provider_request_id, http_status, terminal_result_json,
                created_at, updated_at, completed_at
             FROM image_generation_executions
             WHERE execution_id = ?1",
            [execution_id],
            |row| {
                let profile_revision = row.get::<_, i64>(4)?;
                let http_status = row
                    .get::<_, Option<i64>>(12)?
                    .map(u16::try_from)
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            12,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?;
                Ok(ImageGenerationExecutionJournalRecord {
                    identity: ImageGenerationExecutionIdentityRecord {
                        execution_id: execution_id.to_string(),
                        request_fingerprint: row.get(0)?,
                        safe_request_json: row.get(1)?,
                        profile_id: row.get(2)?,
                        adapter_id: row.get(3)?,
                        profile_revision: u64::try_from(profile_revision).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                4,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?,
                        model_id: row.get(5)?,
                        operation: row.get(6)?,
                    },
                    status: StoredImageGenerationExecutionStatus::parse(&row.get::<_, String>(7)?)?,
                    remote_outcome_unknown: row.get(8)?,
                    provider_succeeded: row.get(9)?,
                    commit_may_have_succeeded: row.get(10)?,
                    provider_request_id: row.get(11)?,
                    http_status,
                    terminal_result_json: row.get(13)?,
                    created_at: row.get(14)?,
                    updated_at: row.get(15)?,
                    completed_at: row.get(16)?,
                    artifact: None,
                })
            },
        )
        .optional()?;
    let Some(mut execution) = execution else {
        return Ok(None);
    };
    execution.artifact = connection
        .query_row(
            "SELECT ordinal, artifact_id, state, storage_relative_path, format, media_type,
                    width, height, size_bytes, sha256, created_at, published_at
             FROM image_generation_artifacts
             WHERE execution_id = ?1 AND ordinal = 0",
            [execution_id],
            |row| {
                let ordinal = u32::try_from(row.get::<_, i64>(0)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
                let width = u32::try_from(row.get::<_, i64>(6)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
                let height = u32::try_from(row.get::<_, i64>(7)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
                let size_bytes = u64::try_from(row.get::<_, i64>(8)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
                Ok(ImageGenerationArtifactJournalRecord {
                    ordinal,
                    artifact_id: row.get(1)?,
                    state: StoredImageGenerationArtifactState::parse(&row.get::<_, String>(2)?)?,
                    storage_relative_path: row.get(3)?,
                    format: row.get(4)?,
                    media_type: row.get(5)?,
                    width,
                    height,
                    size_bytes,
                    sha256: row.get(9)?,
                    created_at: row.get(10)?,
                    published_at: row.get(11)?,
                })
            },
        )
        .optional()?;
    Ok(Some(execution))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    fn identity(execution_id: &str) -> ImageGenerationExecutionIdentityRecord {
        ImageGenerationExecutionIdentityRecord {
            execution_id: execution_id.to_string(),
            request_fingerprint: format!("sha256:{}", "a".repeat(64)),
            safe_request_json: r#"{"schemaVersion":1}"#.to_string(),
            profile_id: "default".to_string(),
            adapter_id: "smartmlSeedream".to_string(),
            profile_revision: 2,
            model_id: "seedream".to_string(),
            operation: "generate".to_string(),
        }
    }

    fn artifact() -> ImageGenerationArtifactJournalRecord {
        ImageGenerationArtifactJournalRecord {
            ordinal: 0,
            artifact_id: format!("sha256:{}", "b".repeat(64)),
            state: StoredImageGenerationArtifactState::Candidate,
            storage_relative_path: format!("objects/{}.png", "b".repeat(64)),
            format: "png".to_string(),
            media_type: "image/png".to_string(),
            width: 2,
            height: 3,
            size_bytes: 100,
            sha256: "b".repeat(64),
            created_at: 0,
            published_at: None,
        }
    }

    fn terminal_json(status: &str) -> String {
        format!(r#"{{"schemaVersion":1,"status":"{status}"}}"#)
    }

    #[test]
    fn claim_is_permanent_and_rejects_identity_reuse() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let first = identity("execution-1");
        assert!(matches!(
            claim_image_generation_execution(&mut connection, &first).unwrap(),
            ImageGenerationExecutionClaimOutcome::Claimed(_)
        ));
        assert!(matches!(
            claim_image_generation_execution(&mut connection, &first).unwrap(),
            ImageGenerationExecutionClaimOutcome::Existing(_)
        ));
        let mut different = first;
        different.model_id = "different".to_string();
        assert!(matches!(
            claim_image_generation_execution(&mut connection, &different).unwrap(),
            ImageGenerationExecutionClaimOutcome::IdentityConflict(_)
        ));
    }

    #[test]
    fn candidate_precedes_published_terminal_receipt() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        claim_image_generation_execution(&mut connection, &identity("execution-2")).unwrap();
        let prepared = prepare_image_generation_artifact(
            &mut connection,
            "execution-2",
            &artifact(),
            Some(&format!("sha256:{}", "c".repeat(32))),
            Some(200),
        )
        .unwrap();
        assert!(matches!(
            prepared,
            ImageGenerationExecutionMutationOutcome::Updated(_)
        ));
        let terminal = ImageGenerationExecutionTerminalUpdate {
            expected_request_fingerprint: format!("sha256:{}", "a".repeat(64)),
            expected_artifact_sha256: Some("b".repeat(64)),
            status: StoredImageGenerationExecutionStatus::Succeeded,
            remote_outcome_unknown: false,
            provider_succeeded: true,
            commit_may_have_succeeded: false,
            provider_request_id: Some(format!("sha256:{}", "c".repeat(32))),
            http_status: Some(200),
            terminal_result_json: terminal_json("succeeded"),
        };
        let result =
            finalize_image_generation_execution(&mut connection, "execution-2", &terminal).unwrap();
        let ImageGenerationExecutionMutationOutcome::Updated(record) = result else {
            panic!("expected updated receipt")
        };
        assert_eq!(
            record.status,
            StoredImageGenerationExecutionStatus::Succeeded
        );
        assert_eq!(
            record.artifact.unwrap().state,
            StoredImageGenerationArtifactState::Published
        );
        assert!(matches!(
            finalize_image_generation_execution(&mut connection, "execution-2", &terminal).unwrap(),
            ImageGenerationExecutionMutationOutcome::AlreadyCurrent(_)
        ));
    }

    #[test]
    fn success_without_a_frozen_candidate_fails_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        claim_image_generation_execution(&mut connection, &identity("execution-3")).unwrap();
        let result = finalize_image_generation_execution(
            &mut connection,
            "execution-3",
            &ImageGenerationExecutionTerminalUpdate {
                expected_request_fingerprint: format!("sha256:{}", "a".repeat(64)),
                expected_artifact_sha256: None,
                status: StoredImageGenerationExecutionStatus::Succeeded,
                remote_outcome_unknown: false,
                provider_succeeded: true,
                commit_may_have_succeeded: false,
                provider_request_id: None,
                http_status: Some(200),
                terminal_result_json: terminal_json("succeeded"),
            },
        )
        .unwrap();
        assert!(matches!(
            result,
            ImageGenerationExecutionMutationOutcome::StateConflict(Some(_))
        ));
    }

    #[test]
    fn indeterminate_commit_can_only_refine_against_the_frozen_artifact() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        claim_image_generation_execution(&mut connection, &identity("execution-4")).unwrap();
        prepare_image_generation_artifact(
            &mut connection,
            "execution-4",
            &artifact(),
            Some(&format!("sha256:{}", "c".repeat(32))),
            Some(200),
        )
        .unwrap();
        let indeterminate = ImageGenerationExecutionTerminalUpdate {
            expected_request_fingerprint: format!("sha256:{}", "a".repeat(64)),
            expected_artifact_sha256: Some("b".repeat(64)),
            status: StoredImageGenerationExecutionStatus::CommitIndeterminate,
            remote_outcome_unknown: false,
            provider_succeeded: true,
            commit_may_have_succeeded: true,
            provider_request_id: Some(format!("sha256:{}", "c".repeat(32))),
            http_status: Some(200),
            terminal_result_json: terminal_json("commitIndeterminate"),
        };
        let ImageGenerationExecutionMutationOutcome::Updated(record) =
            finalize_image_generation_execution(&mut connection, "execution-4", &indeterminate)
                .unwrap()
        else {
            panic!("expected indeterminate terminal receipt")
        };
        assert_eq!(
            record.artifact.unwrap().state,
            StoredImageGenerationArtifactState::Indeterminate
        );

        let reconciled = ImageGenerationExecutionTerminalUpdate {
            expected_request_fingerprint: format!("sha256:{}", "a".repeat(64)),
            expected_artifact_sha256: Some("b".repeat(64)),
            status: StoredImageGenerationExecutionStatus::Succeeded,
            remote_outcome_unknown: false,
            provider_succeeded: true,
            commit_may_have_succeeded: false,
            provider_request_id: Some(format!("sha256:{}", "c".repeat(32))),
            http_status: Some(200),
            terminal_result_json: terminal_json("succeeded"),
        };
        let ImageGenerationExecutionMutationOutcome::Updated(record) =
            finalize_image_generation_execution(&mut connection, "execution-4", &reconciled)
                .unwrap()
        else {
            panic!("expected reconciled terminal receipt")
        };
        assert_eq!(
            record.status,
            StoredImageGenerationExecutionStatus::Succeeded
        );
        assert_eq!(
            record.artifact.unwrap().state,
            StoredImageGenerationArtifactState::Published
        );

        let mut wrong_artifact = reconciled;
        wrong_artifact.expected_artifact_sha256 = Some("d".repeat(64));
        assert!(matches!(
            finalize_image_generation_execution(&mut connection, "execution-4", &wrong_artifact,)
                .unwrap(),
            ImageGenerationExecutionMutationOutcome::StateConflict(Some(_))
        ));
    }
}
