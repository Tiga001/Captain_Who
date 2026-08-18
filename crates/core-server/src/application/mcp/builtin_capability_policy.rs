use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

const POLICY_SCHEMA_VERSION: i64 = 1;
const POLICY_METADATA_SINGLETON: i64 = 1;
const BROWSER_AUTOMATION_ID: &str = "browser_automation";
const BROWSER_AUTOMATION_POLICY_VERSION: u32 = 1;
const MAX_WIRE_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuiltinCapabilityId {
    BrowserAutomation,
}

impl BuiltinCapabilityId {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::BrowserAutomation => BROWSER_AUTOMATION_ID,
        }
    }

    fn parse(value: &str) -> Result<Self, BuiltinCapabilityPolicyError> {
        match value {
            BROWSER_AUTOMATION_ID => Ok(Self::BrowserAutomation),
            _ => Err(BuiltinCapabilityPolicyError::CorruptRecord),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BuiltinCapabilityPolicyRecord {
    pub(crate) capability_id: BuiltinCapabilityId,
    pub(crate) user_allowed: bool,
    pub(crate) policy_version: u32,
    pub(crate) policy_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuiltinCapabilityPolicyError {
    Conflict,
    CorruptRecord,
    InvalidInput,
    StorageUnavailable,
    RevisionExhausted,
}

#[derive(Debug)]
pub(crate) struct SqliteBuiltinCapabilityPolicyStore {
    connection: Mutex<Connection>,
}

impl SqliteBuiltinCapabilityPolicyStore {
    pub(crate) fn open(
        database_path: impl AsRef<Path>,
    ) -> Result<Self, BuiltinCapabilityPolicyError> {
        let connection = Connection::open(database_path)
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
        validate_schema(&connection)?;
        seed_metadata(&connection)?;
        validate_persisted_state(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub(crate) fn snapshot(
        &self,
    ) -> Result<(u64, Vec<BuiltinCapabilityPolicyRecord>), BuiltinCapabilityPolicyError> {
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
        let revision = read_revision(&transaction)?;
        let record = load_policy(&transaction, BuiltinCapabilityId::BrowserAutomation)?
            .unwrap_or_else(default_browser_automation_policy);
        transaction
            .commit()
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
        Ok((revision, vec![record]))
    }

    /// Reads the current durable policy without going through Renderer/RPC DTOs. Runtime
    /// capability dispatch can use this as its final fail-closed `user_allowed` check.
    pub(crate) fn get(
        &self,
        capability_id: BuiltinCapabilityId,
    ) -> Result<BuiltinCapabilityPolicyRecord, BuiltinCapabilityPolicyError> {
        let connection = self.lock_connection()?;
        load_policy(&connection, capability_id).map(|record| {
            record.unwrap_or_else(|| match capability_id {
                BuiltinCapabilityId::BrowserAutomation => default_browser_automation_policy(),
            })
        })
    }

    pub(crate) fn set_allowed(
        &self,
        capability_id: BuiltinCapabilityId,
        expected_policy_revision: u64,
        user_allowed: bool,
    ) -> Result<(u64, BuiltinCapabilityPolicyRecord), BuiltinCapabilityPolicyError> {
        if expected_policy_revision > MAX_WIRE_SAFE_INTEGER {
            return Err(BuiltinCapabilityPolicyError::InvalidInput);
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
        let revision = read_revision(&transaction)?;
        let current = load_policy(&transaction, capability_id)?
            .unwrap_or_else(default_browser_automation_policy);
        if current.policy_revision != expected_policy_revision {
            return Err(BuiltinCapabilityPolicyError::Conflict);
        }
        if current.user_allowed == user_allowed {
            transaction
                .commit()
                .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
            return Ok((revision, current));
        }

        let next_revision = revision
            .checked_add(1)
            .filter(|value| *value <= MAX_WIRE_SAFE_INTEGER)
            .ok_or(BuiltinCapabilityPolicyError::RevisionExhausted)?;
        transaction
            .execute(
                "INSERT INTO mcp_builtin_capability_policies (
                     schema_version, capability_id, user_allowed, policy_version, policy_revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(capability_id) DO UPDATE SET
                     schema_version = excluded.schema_version,
                     user_allowed = excluded.user_allowed,
                     policy_version = excluded.policy_version,
                     policy_revision = excluded.policy_revision",
                params![
                    POLICY_SCHEMA_VERSION,
                    capability_id.as_str(),
                    user_allowed,
                    BROWSER_AUTOMATION_POLICY_VERSION,
                    next_revision,
                ],
            )
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
        transaction
            .execute(
                "UPDATE mcp_builtin_capability_metadata
                 SET revision_watermark = ?1
                 WHERE singleton = ?2 AND schema_version = ?3 AND revision_watermark = ?4",
                params![
                    next_revision,
                    POLICY_METADATA_SINGLETON,
                    POLICY_SCHEMA_VERSION,
                    revision,
                ],
            )
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)
            .and_then(|count| {
                if count == 1 {
                    Ok(())
                } else {
                    Err(BuiltinCapabilityPolicyError::Conflict)
                }
            })?;
        transaction
            .commit()
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
        Ok((
            next_revision,
            BuiltinCapabilityPolicyRecord {
                capability_id,
                user_allowed,
                policy_version: BROWSER_AUTOMATION_POLICY_VERSION,
                policy_revision: next_revision,
            },
        ))
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, BuiltinCapabilityPolicyError> {
        self.connection
            .lock()
            .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)
    }
}

fn validate_schema(connection: &Connection) -> Result<(), BuiltinCapabilityPolicyError> {
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table'
               AND name IN (
                   'mcp_builtin_capability_metadata',
                   'mcp_builtin_capability_policies'
               )",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
    if count == 2 {
        Ok(())
    } else {
        Err(BuiltinCapabilityPolicyError::CorruptRecord)
    }
}

fn seed_metadata(connection: &Connection) -> Result<(), BuiltinCapabilityPolicyError> {
    connection
        .execute(
            "INSERT OR IGNORE INTO mcp_builtin_capability_metadata (
                 singleton, schema_version, revision_watermark
             ) VALUES (?1, ?2, 0)",
            params![POLICY_METADATA_SINGLETON, POLICY_SCHEMA_VERSION],
        )
        .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
    Ok(())
}

fn validate_persisted_state(connection: &Connection) -> Result<(), BuiltinCapabilityPolicyError> {
    let revision = read_revision(connection)?;
    let mut statement = connection
        .prepare(
            "SELECT capability_id, user_allowed, policy_version, policy_revision
             FROM mcp_builtin_capability_policies ORDER BY capability_id",
        )
        .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
    let mut rows = statement
        .query([])
        .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
    while let Some(row) = rows
        .next()
        .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?
    {
        let record = decode_policy_row(row)?;
        if record.policy_revision > revision {
            return Err(BuiltinCapabilityPolicyError::CorruptRecord);
        }
    }
    Ok(())
}

fn read_revision(connection: &Connection) -> Result<u64, BuiltinCapabilityPolicyError> {
    let (schema_version, revision) = connection
        .query_row(
            "SELECT schema_version, revision_watermark
             FROM mcp_builtin_capability_metadata WHERE singleton = ?1",
            params![POLICY_METADATA_SINGLETON],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|_| BuiltinCapabilityPolicyError::CorruptRecord)?;
    if schema_version != POLICY_SCHEMA_VERSION || revision < 0 {
        return Err(BuiltinCapabilityPolicyError::CorruptRecord);
    }
    u64::try_from(revision)
        .ok()
        .filter(|value| *value <= MAX_WIRE_SAFE_INTEGER)
        .ok_or(BuiltinCapabilityPolicyError::CorruptRecord)
}

fn load_policy(
    connection: &Connection,
    capability_id: BuiltinCapabilityId,
) -> Result<Option<BuiltinCapabilityPolicyRecord>, BuiltinCapabilityPolicyError> {
    let values = connection
        .query_row(
            "SELECT capability_id, user_allowed, policy_version, policy_revision
             FROM mcp_builtin_capability_policies WHERE capability_id = ?1",
            params![capability_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|_| BuiltinCapabilityPolicyError::StorageUnavailable)?;
    values.map(decode_policy_values).transpose()
}

fn decode_policy_row(
    row: &rusqlite::Row<'_>,
) -> Result<BuiltinCapabilityPolicyRecord, BuiltinCapabilityPolicyError> {
    decode_policy_values((
        row.get::<_, String>(0)
            .map_err(|_| BuiltinCapabilityPolicyError::CorruptRecord)?,
        row.get::<_, i64>(1)
            .map_err(|_| BuiltinCapabilityPolicyError::CorruptRecord)?,
        row.get::<_, i64>(2)
            .map_err(|_| BuiltinCapabilityPolicyError::CorruptRecord)?,
        row.get::<_, i64>(3)
            .map_err(|_| BuiltinCapabilityPolicyError::CorruptRecord)?,
    ))
}

fn decode_policy_values(
    (capability_id, user_allowed, policy_version, policy_revision): (String, i64, i64, i64),
) -> Result<BuiltinCapabilityPolicyRecord, BuiltinCapabilityPolicyError> {
    let capability_id = BuiltinCapabilityId::parse(&capability_id)?;
    let user_allowed = match user_allowed {
        0 => false,
        1 => true,
        _ => return Err(BuiltinCapabilityPolicyError::CorruptRecord),
    };
    let policy_version = u32::try_from(policy_version)
        .ok()
        .filter(|version| *version == BROWSER_AUTOMATION_POLICY_VERSION)
        .ok_or(BuiltinCapabilityPolicyError::CorruptRecord)?;
    let policy_revision = u64::try_from(policy_revision)
        .ok()
        .filter(|revision| *revision > 0 && *revision <= MAX_WIRE_SAFE_INTEGER)
        .ok_or(BuiltinCapabilityPolicyError::CorruptRecord)?;
    Ok(BuiltinCapabilityPolicyRecord {
        capability_id,
        user_allowed,
        policy_version,
        policy_revision,
    })
}

fn default_browser_automation_policy() -> BuiltinCapabilityPolicyRecord {
    BuiltinCapabilityPolicyRecord {
        capability_id: BuiltinCapabilityId::BrowserAutomation,
        user_allowed: false,
        policy_version: BROWSER_AUTOMATION_POLICY_VERSION,
        policy_revision: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_automation_defaults_off_and_persists_only_policy_state() {
        let directory = tempfile::tempdir().expect("temporary policy directory");
        let path = directory.path().join("storage.sqlite");
        let storage = mycopilot_core::storage::service::StorageService::open(&path)
            .expect("create canonical storage");
        let store = SqliteBuiltinCapabilityPolicyStore::open(&path).expect("open policy store");

        let (revision, policies) = store.snapshot().expect("load default policy");
        assert_eq!(revision, 0);
        assert_eq!(policies, vec![default_browser_automation_policy()]);

        let (revision, enabled) = store
            .set_allowed(BuiltinCapabilityId::BrowserAutomation, 0, true)
            .expect("enable policy");
        assert_eq!(revision, 1);
        assert!(enabled.user_allowed);
        assert_eq!(enabled.policy_revision, 1);
        assert!(matches!(
            store.set_allowed(BuiltinCapabilityId::BrowserAutomation, 0, false),
            Err(BuiltinCapabilityPolicyError::Conflict)
        ));
        drop(store);
        drop(storage);

        let reopened =
            SqliteBuiltinCapabilityPolicyStore::open(&path).expect("reopen policy store");
        assert_eq!(
            reopened.snapshot().expect("load persisted policy"),
            (1, vec![enabled])
        );

        let connection = Connection::open(&path).expect("inspect policy schema");
        let columns = connection
            .prepare("PRAGMA table_info(mcp_builtin_capability_policies)")
            .expect("prepare policy columns")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query policy columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect policy columns");
        assert_eq!(
            columns,
            [
                "schema_version",
                "capability_id",
                "user_allowed",
                "policy_version",
                "policy_revision"
            ]
        );
    }
}
