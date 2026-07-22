use crate::storage::models::ImageGenerationProfileRecord;
use crate::storage::now_ms;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub const IMAGE_GENERATION_PROFILE_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_IMAGE_GENERATION_PROFILE_ID: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageGenerationProfileCompareAndSetOutcome {
    Updated(ImageGenerationProfileRecord),
    Conflict(Option<ImageGenerationProfileRecord>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageGenerationCredentialStageOutcome {
    Staged,
    Conflict(Option<ImageGenerationProfileRecord>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationCredentialStagingRecord {
    pub credential_ref: String,
    pub is_active: bool,
}

pub fn load_image_generation_profile(
    connection: &Connection,
    profile_id: &str,
) -> rusqlite::Result<Option<ImageGenerationProfileRecord>> {
    query_profile(connection, profile_id)
}

/// Atomically creates or replaces one credential-free provider profile.
///
/// Generation zero represents an absent profile. The repository owns generation and timestamps;
/// callers cannot roll either value backwards through the supplied replacement record.
pub fn compare_and_set_image_generation_profile(
    connection: &mut Connection,
    profile_id: &str,
    expected_generation: u64,
    replacement: &ImageGenerationProfileRecord,
) -> rusqlite::Result<ImageGenerationProfileCompareAndSetOutcome> {
    compare_and_set_image_generation_profile_at(
        connection,
        profile_id,
        expected_generation,
        replacement,
        now_ms(),
    )
}

fn compare_and_set_image_generation_profile_at(
    connection: &mut Connection,
    profile_id: &str,
    expected_generation: u64,
    replacement: &ImageGenerationProfileRecord,
    clock_timestamp: i64,
) -> rusqlite::Result<ImageGenerationProfileCompareAndSetOutcome> {
    let transaction = connection.transaction()?;
    let current = query_profile_transaction(&transaction, profile_id)?;
    let actual_generation = current.as_ref().map_or(0, |record| record.generation);
    if actual_generation != expected_generation {
        transaction.rollback()?;
        return Ok(ImageGenerationProfileCompareAndSetOutcome::Conflict(
            current,
        ));
    }

    let created_at = current
        .as_ref()
        .map_or(clock_timestamp, |record| record.created_at);
    // Wall clocks can move backwards (for example after an NTP correction). Keep the database
    // invariant `updated_at >= created_at` without trusting timestamps supplied by callers.
    let updated_at = clock_timestamp.max(created_at);
    let generation = actual_generation.checked_add(1).ok_or_else(|| {
        rusqlite::Error::ToSqlConversionFailure(
            "image generation profile generation overflow".into(),
        )
    })?;
    let generation_i64 = i64::try_from(generation)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;

    transaction.execute(
        "
        INSERT INTO image_generation_profiles (
            id,
            schema_version,
            adapter_id,
            endpoint_url,
            model_id,
            credential_ref,
            enabled,
            text_to_image,
            image_to_image,
            default_size_preset,
            default_watermark,
            generation,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10, ?11, ?12, ?13)
        ON CONFLICT(id) DO UPDATE SET
            schema_version = excluded.schema_version,
            adapter_id = excluded.adapter_id,
            endpoint_url = excluded.endpoint_url,
            model_id = excluded.model_id,
            credential_ref = excluded.credential_ref,
            enabled = excluded.enabled,
            text_to_image = 1,
            image_to_image = excluded.image_to_image,
            default_size_preset = excluded.default_size_preset,
            default_watermark = excluded.default_watermark,
            generation = excluded.generation,
            updated_at = excluded.updated_at
        ",
        params![
            profile_id,
            IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
            &replacement.adapter_id,
            &replacement.endpoint_url,
            &replacement.model_id,
            &replacement.credential_ref,
            replacement.enabled,
            replacement.image_to_image,
            &replacement.default_size_preset,
            replacement.default_watermark,
            generation_i64,
            created_at,
            updated_at,
        ],
    )?;

    if let Some(previous_reference) = current
        .as_ref()
        .and_then(|record| record.credential_ref.as_deref())
        .filter(|previous| Some(*previous) != replacement.credential_ref.as_deref())
    {
        transaction.execute(
            "INSERT OR IGNORE INTO image_generation_credential_cleanup (
                credential_ref, created_at
             ) VALUES (?1, ?2)",
            params![previous_reference, clock_timestamp],
        )?;
    }

    let stored = query_profile_transaction(&transaction, profile_id)?
        .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;
    transaction.commit()?;
    Ok(ImageGenerationProfileCompareAndSetOutcome::Updated(stored))
}

/// Records a crash-recoverable credential write before secret material enters the native store.
pub fn stage_image_generation_credential(
    connection: &mut Connection,
    profile_id: &str,
    expected_generation: u64,
    credential_ref: &str,
) -> rusqlite::Result<ImageGenerationCredentialStageOutcome> {
    let transaction = connection.transaction()?;
    let current = query_profile_transaction(&transaction, profile_id)?;
    let actual_generation = current.as_ref().map_or(0, |record| record.generation);
    if actual_generation != expected_generation {
        transaction.rollback()?;
        return Ok(ImageGenerationCredentialStageOutcome::Conflict(current));
    }
    let expected_generation = i64::try_from(expected_generation)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    transaction.execute(
        "INSERT INTO image_generation_credential_staging (
            credential_ref, profile_id, expected_generation, created_at
         ) VALUES (?1, ?2, ?3, ?4)",
        params![credential_ref, profile_id, expected_generation, now_ms()],
    )?;
    transaction.commit()?;
    Ok(ImageGenerationCredentialStageOutcome::Staged)
}

pub fn complete_image_generation_credential_staging(
    connection: &Connection,
    credential_ref: &str,
) -> rusqlite::Result<bool> {
    connection
        .execute(
            "DELETE FROM image_generation_credential_staging WHERE credential_ref = ?1",
            [credential_ref],
        )
        .map(|changed| changed > 0)
}

pub fn list_image_generation_credential_staging(
    connection: &Connection,
) -> rusqlite::Result<Vec<ImageGenerationCredentialStagingRecord>> {
    let mut statement = connection.prepare(
        "SELECT
            staging.credential_ref,
            EXISTS (
                SELECT 1
                FROM image_generation_profiles AS profile
                WHERE profile.id = staging.profile_id
                  AND profile.credential_ref = staging.credential_ref
            ) AS is_active
         FROM image_generation_credential_staging AS staging
         ORDER BY staging.created_at ASC, staging.credential_ref ASC",
    )?;
    let records = statement
        .query_map([], |row| {
            Ok(ImageGenerationCredentialStagingRecord {
                credential_ref: row.get(0)?,
                is_active: row.get(1)?,
            })
        })?
        .collect();
    records
}

pub fn list_image_generation_credential_cleanup(
    connection: &Connection,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT credential_ref
         FROM image_generation_credential_cleanup
         ORDER BY created_at ASC, credential_ref ASC",
    )?;
    let references = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>();
    references
}

pub fn complete_image_generation_credential_cleanup(
    connection: &Connection,
    credential_ref: &str,
) -> rusqlite::Result<bool> {
    connection
        .execute(
            "DELETE FROM image_generation_credential_cleanup WHERE credential_ref = ?1",
            [credential_ref],
        )
        .map(|changed| changed > 0)
}

fn query_profile_transaction(
    transaction: &Transaction<'_>,
    profile_id: &str,
) -> rusqlite::Result<Option<ImageGenerationProfileRecord>> {
    query_profile(transaction, profile_id)
}

fn query_profile(
    connection: &Connection,
    profile_id: &str,
) -> rusqlite::Result<Option<ImageGenerationProfileRecord>> {
    connection
        .query_row(
            "
            SELECT
                id,
                schema_version,
                adapter_id,
                endpoint_url,
                model_id,
                credential_ref,
                enabled,
                text_to_image,
                image_to_image,
                default_size_preset,
                default_watermark,
                generation,
                created_at,
                updated_at
            FROM image_generation_profiles
            WHERE id = ?1
            ",
            [profile_id],
            |row| {
                Ok(ImageGenerationProfileRecord {
                    id: row.get(0)?,
                    schema_version: row.get(1)?,
                    adapter_id: row.get(2)?,
                    endpoint_url: row.get(3)?,
                    model_id: row.get(4)?,
                    credential_ref: row.get(5)?,
                    enabled: row.get(6)?,
                    text_to_image: row.get(7)?,
                    image_to_image: row.get(8)?,
                    default_size_preset: row.get(9)?,
                    default_watermark: row.get(10)?,
                    generation: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                })
            },
        )
        .optional()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    fn profile() -> ImageGenerationProfileRecord {
        ImageGenerationProfileRecord {
            id: DEFAULT_IMAGE_GENERATION_PROFILE_ID.to_string(),
            schema_version: IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
            adapter_id: "smartmlSeedream".to_string(),
            endpoint_url: "https://zju.smartml.cn/userapi/v1/images/generations".to_string(),
            model_id: "doubao-seedream-4-0-250828".to_string(),
            credential_ref: Some("image-generation/test/credential-1".to_string()),
            enabled: false,
            text_to_image: true,
            image_to_image: false,
            default_size_preset: "2K".to_string(),
            default_watermark: true,
            generation: 99,
            created_at: 99,
            updated_at: 99,
        }
    }

    #[test]
    fn compare_and_set_owns_generation_and_detects_stale_writes() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();

        let first = compare_and_set_image_generation_profile(
            &mut connection,
            DEFAULT_IMAGE_GENERATION_PROFILE_ID,
            0,
            &profile(),
        )
        .unwrap();
        let ImageGenerationProfileCompareAndSetOutcome::Updated(first) = first else {
            panic!("first write should succeed");
        };
        assert_eq!(first.generation, 1);
        assert!(first.text_to_image);

        let stale = compare_and_set_image_generation_profile(
            &mut connection,
            DEFAULT_IMAGE_GENERATION_PROFILE_ID,
            0,
            &profile(),
        )
        .unwrap();
        let ImageGenerationProfileCompareAndSetOutcome::Conflict(Some(current)) = stale else {
            panic!("stale write should report current state");
        };
        assert_eq!(current.generation, 1);
    }

    #[test]
    fn compare_and_set_clamps_updated_at_when_wall_clock_moves_backwards() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();

        let first = compare_and_set_image_generation_profile_at(
            &mut connection,
            DEFAULT_IMAGE_GENERATION_PROFILE_ID,
            0,
            &profile(),
            10_000,
        )
        .unwrap();
        let ImageGenerationProfileCompareAndSetOutcome::Updated(first) = first else {
            panic!("first write should succeed");
        };
        assert_eq!(first.created_at, 10_000);
        assert_eq!(first.updated_at, 10_000);

        let second = compare_and_set_image_generation_profile_at(
            &mut connection,
            DEFAULT_IMAGE_GENERATION_PROFILE_ID,
            first.generation,
            &profile(),
            9_000,
        )
        .unwrap();
        let ImageGenerationProfileCompareAndSetOutcome::Updated(second) = second else {
            panic!("second write should succeed despite the clock rollback");
        };
        assert_eq!(second.generation, 2);
        assert_eq!(second.created_at, 10_000);
        assert_eq!(second.updated_at, 10_000);
    }

    #[test]
    fn database_rejects_disabling_text_to_image() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();

        let result = connection.execute(
            "INSERT INTO image_generation_profiles (
                id, schema_version, adapter_id, endpoint_url, model_id, credential_ref,
                enabled, text_to_image, image_to_image, default_size_preset,
                default_watermark, generation, created_at, updated_at
             ) VALUES ('default', 1, 'smartmlSeedream', '', '', NULL, 0, 0, 0, '2K', 1, 1, 0, 0)",
            [],
        );

        assert!(result.is_err());
    }
}
