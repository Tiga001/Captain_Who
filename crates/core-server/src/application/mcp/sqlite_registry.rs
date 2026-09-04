use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use mycopilot_mcp_client::{
    allocate_model_namespace, config_digest, McpApprovalMode, McpConfigDigest, McpConfigEpoch,
    McpError, McpModelNamespace, McpRegistry, McpRegistryChange, McpRegistryChangeKind,
    McpRegistryEntry, McpRegistryMutation, McpRegistrySubscription, McpServerConfig, McpServerId,
    McpServerScope, McpStdioConfig, McpTransportConfig, McpTrustLevel,
};
use rusqlite::{
    params, types::ValueRef, Connection, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

#[path = "sqlite_registry/launch_identity.rs"]
mod launch_identity;
#[path = "sqlite_registry/reconciliation.rs"]
mod reconciliation;
#[path = "sqlite_registry/schema.rs"]
mod schema;
#[path = "sqlite_registry/storage.rs"]
mod storage;
#[path = "sqlite_registry/validation.rs"]
mod validation;

pub(crate) use launch_identity::{
    compute_launch_file_identity_digest, compute_launch_spec_digest,
    launch_authorization_identity_is_valid, launch_authorization_is_valid,
    prepare_launch_file_identity,
};
use reconciliation::reconcile_startup;
use schema::prepare_schema;
use storage::*;
use validation::*;

#[cfg(test)]
#[path = "sqlite_registry/tests.rs"]
mod tests;

const REGISTRY_SCHEMA_VERSION: i64 = 1;
const REGISTRY_METADATA_SINGLETON: i64 = 1;
const REGISTRY_CHANGE_CAPACITY: usize = 128;
const MODEL_NAMESPACE_SCHEMA_VERSION: i64 = 1;
pub(crate) const MCP_REGISTRY_MAX_SERVERS: usize = 1024;
const MAX_WIRE_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub(crate) const MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION: u32 = 2;
pub(crate) const MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION: u32 = 3;
const SOURCE_USER_MANUAL: &str = "user_manual";
const RECORD_STATE_ACTIVE: &str = "active";
const RECORD_STATE_INVALID: &str = "invalid";
const SAFE_ERROR_RECONCILED: &str = "startup_record_reconciled";
const SAFE_ERROR_INVALID: &str = "invalid_persisted_record";
const LAUNCH_DIGEST_DOMAIN: &[u8] = b"mycopilot-mcp-launch-spec-v1\0";
const LAUNCH_FILE_IDENTITY_DOMAIN: &[u8] = b"mycopilot-mcp-launch-file-identity-v2\0";
const MAX_LAUNCH_CODE_INPUTS: usize = 16;
const MAX_LAUNCH_CONTENT_HASH_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LAUNCH_CONTENT_HASH_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const LAUNCH_IDENTITY_READ_BUFFER_BYTES: usize = 64 * 1024;

const MAX_DISPLAY_NAME_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 4096;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
const MAX_ARGUMENTS_TOTAL_BYTES: usize = 64 * 1024;
const MAX_CONNECT_TIMEOUT_MS: u64 = 10_000;
const MAX_REQUEST_TIMEOUT_MS: u64 = 300_000;
const MAX_SHUTDOWN_TIMEOUT_MS: u64 = 2_000;

const SELECT_COLUMNS: &str = "
    schema_version,
    server_id,
    display_name,
    scope_kind,
    source_kind,
    transport_kind,
    executable,
    arguments_json,
    cwd,
    enabled,
    trust,
    approval_mode,
    connect_timeout_ms,
    request_timeout_ms,
    shutdown_timeout_ms,
    config_digest,
    config_epoch,
    registry_revision,
    launch_spec_digest,
    authorized_launch_spec_digest,
    authorized_config_epoch,
    authorized_config_digest,
    authorization_format_version,
    authorization_policy_version,
    authorized_at,
    record_state,
    safe_error_code,
    created_at,
    updated_at,
    (
        SELECT namespace.model_namespace
        FROM mcp_registry_model_namespaces AS namespace
        WHERE namespace.server_id = mcp_registry_servers.server_id
    ) AS model_namespace
";

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct McpLaunchSpecDigest(String);

impl McpLaunchSpecDigest {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for McpLaunchSpecDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("McpLaunchSpecDigest")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for McpLaunchSpecDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for McpLaunchSpecDigest {
    type Err = McpRegistryPersistenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if is_sha256_hex(value) {
            Ok(Self(value.to_string()))
        } else {
            Err(McpRegistryPersistenceError::CorruptRecord)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpRegistryMutationPrecondition {
    pub(crate) server_id: McpServerId,
    pub(crate) expected_revision: u64,
    pub(crate) expected_config_epoch: McpConfigEpoch,
    pub(crate) expected_config_digest: McpConfigDigest,
}

impl McpRegistryMutationPrecondition {
    pub(crate) fn from_entry(entry: &McpRegistryEntry) -> Self {
        Self {
            server_id: entry.config.id,
            expected_revision: entry.revision,
            expected_config_epoch: entry.config_epoch,
            expected_config_digest: entry.config_digest.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpLaunchAuthorizationRecord {
    pub(crate) authorization_format_version: u32,
    pub(crate) server_id: McpServerId,
    pub(crate) launch_spec_digest: McpLaunchSpecDigest,
    /// Digest of the canonical executable, cwd, and every code-bearing argv
    /// entry, including stable filesystem identity and complete file bytes at
    /// authorization time.
    ///
    /// The legacy SQLite column is named `authorized_launch_spec_digest`.
    /// Format v2 deliberately stores this stronger identity there; the
    /// logical launch-spec digest remains in the adjacent non-authority
    /// `launch_spec_digest` column.
    pub(crate) file_identity_digest: McpLaunchSpecDigest,
    /// The current configuration identity bound to this exact launch
    /// authorization. Host-owned non-launch mutations may rebind these fields
    /// only while the physical launch digest remains unchanged.
    pub(crate) authored_config_epoch: McpConfigEpoch,
    pub(crate) authored_config_digest: McpConfigDigest,
    pub(crate) authorization_policy_version: u32,
    pub(crate) authorized_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpPersistedRegistryRecord {
    pub(crate) entry: McpRegistryEntry,
    pub(crate) source: McpServerSource,
    pub(crate) launch_spec_digest: McpLaunchSpecDigest,
    pub(crate) launch_authorization: Option<McpLaunchAuthorizationRecord>,
    pub(crate) safe_error_code: Option<String>,
    pub(crate) created_at_ms: i64,
    pub(crate) updated_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpServerSource {
    UserManual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct McpStartupReconciliationReport {
    pub(crate) recovered_records: usize,
    pub(crate) quarantined_records: usize,
    pub(crate) revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpRegistryPersistenceError {
    InvalidConfig,
    NotFound,
    Conflict,
    CapacityExceeded,
    AuthorizationRequired,
    CorruptRecord,
    DevelopmentStorageSchemaResetRequired,
    StorageUnavailable,
    RevisionExhausted,
}

impl McpRegistryPersistenceError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::InvalidConfig => "invalid_config",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::CapacityExceeded => "capacity_exceeded",
            Self::AuthorizationRequired => "authorization_required",
            Self::CorruptRecord => "corrupt_record",
            Self::DevelopmentStorageSchemaResetRequired => {
                "development_storage_schema_reset_required"
            }
            Self::StorageUnavailable => "storage_unavailable",
            Self::RevisionExhausted => "revision_exhausted",
        }
    }

    fn into_mcp_error(self) -> McpError {
        match self {
            Self::InvalidConfig => McpError::config("MCP Registry configuration is invalid"),
            Self::NotFound => McpError::config("MCP Registry server was not found"),
            Self::Conflict => McpError::config("MCP Registry mutation conflicted"),
            Self::CapacityExceeded => McpError::config("MCP Registry server limit was reached"),
            Self::AuthorizationRequired => {
                McpError::config("MCP server launch authorization is required")
            }
            Self::CorruptRecord => {
                McpError::config("MCP Registry contains an invalid persisted record")
            }
            Self::DevelopmentStorageSchemaResetRequired => McpError::config(
                "The development storage schema must be reset before MCP Registry startup",
            ),
            Self::StorageUnavailable => McpError::protocol("MCP Registry storage is unavailable"),
            Self::RevisionExhausted => McpError::protocol("MCP Registry revision is exhausted"),
        }
    }
}

impl fmt::Display for McpRegistryPersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for McpRegistryPersistenceError {}

impl From<rusqlite::Error> for McpRegistryPersistenceError {
    fn from(_: rusqlite::Error) -> Self {
        Self::StorageUnavailable
    }
}

#[derive(Debug)]
pub(crate) struct SqliteMcpRegistry {
    connection: Mutex<Connection>,
    changes: broadcast::Sender<McpRegistryChange>,
}

impl SqliteMcpRegistry {
    pub(crate) fn open(
        database_path: impl AsRef<Path>,
    ) -> Result<Self, McpRegistryPersistenceError> {
        let mut connection = Connection::open(database_path)
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        prepare_schema(&mut connection)?;
        reconcile_startup(&mut connection)?;
        list_records(&connection)?;
        let (changes, _) = broadcast::channel(REGISTRY_CHANGE_CAPACITY);
        Ok(Self {
            connection: Mutex::new(connection),
            changes,
        })
    }

    pub(crate) fn current_revision(&self) -> Result<u64, McpRegistryPersistenceError> {
        let connection = self.lock_connection()?;
        read_revision(&connection)
    }

    pub(crate) fn get_persisted(
        &self,
        server_id: McpServerId,
    ) -> Result<Option<McpPersistedRegistryRecord>, McpRegistryPersistenceError> {
        let connection = self.lock_connection()?;
        load_record(&connection, server_id)
    }

    pub(crate) fn list_persisted(
        &self,
    ) -> Result<Vec<McpPersistedRegistryRecord>, McpRegistryPersistenceError> {
        let connection = self.lock_connection()?;
        list_records(&connection)
    }

    /// Reads the Registry watermark and active records from one SQLite snapshot.
    ///
    /// Keeping these values in the same read transaction prevents management
    /// clients from observing records from one revision paired with a newer
    /// watermark.
    pub(crate) fn snapshot(
        &self,
    ) -> Result<(u64, Vec<McpPersistedRegistryRecord>), McpRegistryPersistenceError> {
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let revision = read_revision(&transaction)?;
        let records = list_records(&transaction)?;
        transaction
            .commit()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        Ok((revision, records))
    }

    pub(crate) fn add_persisted(
        &self,
        config: McpServerConfig,
        preserved_model_namespace: Option<McpModelNamespace>,
    ) -> Result<McpRegistryEntry, McpRegistryPersistenceError> {
        let config = normalize_new_config(config)?;
        let (entry, change) = {
            let mut connection = self.lock_connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            if load_record(&transaction, config.id)?.is_some() {
                return Err(McpRegistryPersistenceError::Conflict);
            }
            let record = insert_record(&transaction, config, preserved_model_namespace)?;
            let change = registry_change(McpRegistryChangeKind::Added, &record.entry);
            transaction
                .commit()
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            (record.entry, change)
        };
        self.publish(change);
        Ok(entry)
    }

    pub(crate) fn update_with_precondition(
        &self,
        precondition: &McpRegistryMutationPrecondition,
        config: McpServerConfig,
    ) -> Result<McpRegistryMutation, McpRegistryPersistenceError> {
        let (mutation, change) = {
            let mut connection = self.lock_connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            let existing = load_record(&transaction, precondition.server_id)?
                .ok_or(McpRegistryPersistenceError::NotFound)?;
            ensure_precondition(&existing.entry, precondition)?;
            let result = update_existing(&transaction, existing, config)?;
            transaction
                .commit()
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            result
        };
        if let Some(change) = change {
            self.publish(change);
        }
        Ok(mutation)
    }

    pub(crate) fn set_enabled(
        &self,
        precondition: &McpRegistryMutationPrecondition,
        enabled: bool,
    ) -> Result<McpRegistryEntry, McpRegistryPersistenceError> {
        let live_authorization_valid = if enabled {
            let existing = self
                .get_persisted(precondition.server_id)?
                .ok_or(McpRegistryPersistenceError::NotFound)?;
            ensure_precondition(&existing.entry, precondition)?;
            launch_authorization_is_valid(&existing)
        } else {
            false
        };
        let (entry, change) = {
            let mut connection = self.lock_connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            let existing = load_record(&transaction, precondition.server_id)?
                .ok_or(McpRegistryPersistenceError::NotFound)?;
            ensure_precondition(&existing.entry, precondition)?;
            if enabled
                && (!live_authorization_valid || !launch_authorization_identity_is_valid(&existing))
            {
                return Err(McpRegistryPersistenceError::AuthorizationRequired);
            }
            if existing.entry.config.enabled == enabled {
                transaction
                    .commit()
                    .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
                (existing.entry, None)
            } else {
                let mut config = existing.entry.config.clone();
                config.enabled = enabled;
                let record = commit_updated_record(
                    &transaction,
                    &existing,
                    config,
                    existing.launch_authorization.clone(),
                    None,
                )?;
                let change = registry_change(McpRegistryChangeKind::Updated, &record.entry);
                transaction
                    .commit()
                    .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
                (record.entry, Some(change))
            }
        };
        if let Some(change) = change {
            self.publish(change);
        }
        Ok(entry)
    }

    pub(crate) fn authorize_launch(
        &self,
        precondition: &McpRegistryMutationPrecondition,
        expected_launch_spec_digest: &McpLaunchSpecDigest,
        expected_file_identity_digest: &McpLaunchSpecDigest,
        authorization_policy_version: u32,
        authorized_at_ms: i64,
    ) -> Result<McpLaunchAuthorizationRecord, McpRegistryPersistenceError> {
        if authorization_policy_version != MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION
            || authorized_at_ms < 0
        {
            return Err(McpRegistryPersistenceError::InvalidConfig);
        }
        // Filesystem validation must not run while SQLite holds an IMMEDIATE
        // transaction. The transaction below rechecks the Registry CAS; the
        // final connector boundary rechecks this identity again before spawn.
        let verified_file_identity_digest = {
            let existing = self
                .get_persisted(precondition.server_id)?
                .ok_or(McpRegistryPersistenceError::NotFound)?;
            ensure_precondition(&existing.entry, precondition)?;
            if &existing.launch_spec_digest != expected_launch_spec_digest {
                return Err(McpRegistryPersistenceError::Conflict);
            }
            let live = compute_launch_file_identity_digest(
                &existing.entry.config,
                expected_launch_spec_digest,
            )?;
            if &live != expected_file_identity_digest {
                return Err(McpRegistryPersistenceError::Conflict);
            }
            live
        };
        let (authorization, change) = {
            let mut connection = self.lock_connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            let existing = load_record(&transaction, precondition.server_id)?
                .ok_or(McpRegistryPersistenceError::NotFound)?;
            ensure_precondition(&existing.entry, precondition)?;
            if &existing.launch_spec_digest != expected_launch_spec_digest {
                return Err(McpRegistryPersistenceError::Conflict);
            }
            if let Some(current) = &existing.launch_authorization {
                if current.launch_spec_digest == *expected_launch_spec_digest
                    && current.file_identity_digest == verified_file_identity_digest
                    && launch_authorization_identity_is_valid(&existing)
                    && current.server_id == existing.entry.config.id
                    && current.authored_config_epoch == existing.entry.config_epoch
                    && current.authored_config_digest == existing.entry.config_digest
                    && current.authorization_format_version
                        == MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION
                    && current.authorization_policy_version == authorization_policy_version
                    && existing.entry.config.trust == McpTrustLevel::UserApproved
                {
                    transaction
                        .commit()
                        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
                    return Ok(current.clone());
                }
            }
            let mut config = existing.entry.config.clone();
            config.trust = McpTrustLevel::UserApproved;
            let record = commit_updated_record(
                &transaction,
                &existing,
                config,
                None,
                Some((
                    authorization_policy_version,
                    authorized_at_ms,
                    verified_file_identity_digest,
                )),
            )?;
            let authorization = record
                .launch_authorization
                .clone()
                .ok_or(McpRegistryPersistenceError::StorageUnavailable)?;
            let change = registry_change(McpRegistryChangeKind::Updated, &record.entry);
            transaction
                .commit()
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            (authorization, change)
        };
        self.publish(change);
        Ok(authorization)
    }

    pub(crate) fn remove_with_precondition(
        &self,
        precondition: &McpRegistryMutationPrecondition,
    ) -> Result<McpRegistryEntry, McpRegistryPersistenceError> {
        let (entry, change) = {
            let mut connection = self.lock_connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            let existing = load_record(&transaction, precondition.server_id)?
                .ok_or(McpRegistryPersistenceError::NotFound)?;
            ensure_precondition(&existing.entry, precondition)?;
            let (entry, change) = remove_record(&transaction, existing)?;
            transaction
                .commit()
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            (entry, change)
        };
        self.publish(change);
        Ok(entry)
    }

    #[allow(dead_code)] // `open` runs this automatically; exposed for controlled host recovery.
    pub(crate) fn startup_reconcile(
        &self,
    ) -> Result<McpStartupReconciliationReport, McpRegistryPersistenceError> {
        let mut connection = self.lock_connection()?;
        reconcile_startup(&mut connection)
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, McpRegistryPersistenceError> {
        self.connection
            .lock()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
    }

    fn publish(&self, change: McpRegistryChange) {
        let _ = self.changes.send(change);
    }
}

impl McpRegistry for SqliteMcpRegistry {
    fn add(&self, config: McpServerConfig) -> Result<McpRegistryEntry, McpError> {
        self.add_persisted(config, None)
            .map_err(McpRegistryPersistenceError::into_mcp_error)
    }

    fn upsert(&self, config: McpServerConfig) -> Result<McpRegistryMutation, McpError> {
        let result = (|| {
            let (mutation, change) = {
                let mut connection = self.lock_connection()?;
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
                let result = if let Some(existing) = load_record(&transaction, config.id)? {
                    update_existing(&transaction, existing, config)?
                } else {
                    let config = normalize_new_config(config)?;
                    let record = insert_record(&transaction, config, None)?;
                    let change = registry_change(McpRegistryChangeKind::Added, &record.entry);
                    (McpRegistryMutation::Added(record.entry), Some(change))
                };
                transaction
                    .commit()
                    .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
                result
            };
            if let Some(change) = change {
                self.publish(change);
            }
            Ok(mutation)
        })();
        result.map_err(McpRegistryPersistenceError::into_mcp_error)
    }

    fn remove(&self, server_id: McpServerId) -> Result<Option<McpRegistryEntry>, McpError> {
        let result = (|| {
            let outcome = {
                let mut connection = self.lock_connection()?;
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
                let Some(existing) = load_record(&transaction, server_id)? else {
                    transaction
                        .commit()
                        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
                    return Ok(None);
                };
                let outcome = remove_record(&transaction, existing)?;
                transaction
                    .commit()
                    .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
                Some(outcome)
            };
            if let Some((entry, change)) = outcome {
                self.publish(change);
                Ok(Some(entry))
            } else {
                Ok(None)
            }
        })();
        result.map_err(McpRegistryPersistenceError::into_mcp_error)
    }

    fn get(&self, server_id: McpServerId) -> Result<Option<McpRegistryEntry>, McpError> {
        self.get_persisted(server_id)
            .map(|record| record.map(|record| record.entry))
            .map_err(McpRegistryPersistenceError::into_mcp_error)
    }

    fn list(&self) -> Result<Vec<McpRegistryEntry>, McpError> {
        self.list_persisted()
            .map(|records| records.into_iter().map(|record| record.entry).collect())
            .map_err(McpRegistryPersistenceError::into_mcp_error)
    }

    fn subscribe(&self) -> McpRegistrySubscription {
        McpRegistrySubscription::from_sender(&self.changes)
    }
}

fn insert_record(
    transaction: &Transaction<'_>,
    config: McpServerConfig,
    preserved_model_namespace: Option<McpModelNamespace>,
) -> Result<McpPersistedRegistryRecord, McpRegistryPersistenceError> {
    ensure_registry_capacity(transaction)?;
    let occupied_namespaces = load_model_namespaces(transaction)?;
    let model_namespace = match preserved_model_namespace {
        Some(namespace) => {
            if occupied_namespaces.contains(&namespace) {
                return Err(McpRegistryPersistenceError::Conflict);
            }
            namespace
        }
        None => {
            allocate_model_namespace(&config.display_name, config.id, occupied_namespaces.iter())
                .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?
        }
    };
    let revision = next_revision(transaction)?;
    let config_epoch = McpConfigEpoch::new();
    let config_digest =
        config_digest(&config).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let launch_spec_digest = compute_launch_spec_digest(&config)?;
    let now = now_ms()?;
    let record = McpPersistedRegistryRecord {
        entry: McpRegistryEntry {
            config,
            model_namespace,
            config_digest,
            config_epoch,
            revision,
        },
        source: McpServerSource::UserManual,
        launch_spec_digest,
        launch_authorization: None,
        safe_error_code: None,
        created_at_ms: now,
        updated_at_ms: now,
    };
    insert_row(transaction, &record)?;
    Ok(record)
}

fn ensure_registry_capacity(
    transaction: &Transaction<'_>,
) -> Result<(), McpRegistryPersistenceError> {
    let active_records = transaction
        .query_row(
            "SELECT COUNT(*) FROM mcp_registry_servers WHERE record_state = 'active'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let active_records =
        usize::try_from(active_records).map_err(|_| McpRegistryPersistenceError::CorruptRecord)?;
    if active_records >= MCP_REGISTRY_MAX_SERVERS {
        return Err(McpRegistryPersistenceError::CapacityExceeded);
    }
    Ok(())
}

fn load_model_namespaces(
    transaction: &Transaction<'_>,
) -> Result<std::collections::BTreeSet<McpModelNamespace>, McpRegistryPersistenceError> {
    let mut statement = transaction
        .prepare(
            "SELECT model_namespace
             FROM mcp_registry_model_namespaces
             ORDER BY model_namespace",
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let values = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    values
        .into_iter()
        .map(|value| {
            McpModelNamespace::from_str(&value)
                .map_err(|_| McpRegistryPersistenceError::CorruptRecord)
        })
        .collect()
}

fn update_existing(
    transaction: &Transaction<'_>,
    existing: McpPersistedRegistryRecord,
    config: McpServerConfig,
) -> Result<(McpRegistryMutation, Option<McpRegistryChange>), McpRegistryPersistenceError> {
    if config.id != existing.entry.config.id {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    let mut config = normalize_config(config)?;
    if config.trust != existing.entry.config.trust
        || config.enabled != existing.entry.config.enabled
    {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    let proposed_digest =
        config_digest(&config).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    if proposed_digest == existing.entry.config_digest {
        return Ok((McpRegistryMutation::Unchanged(existing.entry), None));
    }
    let proposed_launch_digest = compute_launch_spec_digest(&config)?;
    let launch_changed = proposed_launch_digest != existing.launch_spec_digest;
    let authorization = if launch_changed {
        config.enabled = false;
        config.trust = McpTrustLevel::Untrusted;
        None
    } else {
        existing.launch_authorization.clone()
    };
    let record = commit_updated_record(transaction, &existing, config, authorization, None)?;
    let change = registry_change(McpRegistryChangeKind::Updated, &record.entry);
    Ok((McpRegistryMutation::Updated(record.entry), Some(change)))
}

fn commit_updated_record(
    transaction: &Transaction<'_>,
    existing: &McpPersistedRegistryRecord,
    config: McpServerConfig,
    authorization: Option<McpLaunchAuthorizationRecord>,
    new_authorization: Option<(u32, i64, McpLaunchSpecDigest)>,
) -> Result<McpPersistedRegistryRecord, McpRegistryPersistenceError> {
    let config = normalize_config(config)?;
    let revision = next_revision(transaction)?;
    let config_epoch = McpConfigEpoch::new();
    let config_digest =
        config_digest(&config).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let launch_spec_digest = compute_launch_spec_digest(&config)?;
    let authorization =
        if let Some((policy_version, authorized_at_ms, file_identity_digest)) = new_authorization {
            Some(McpLaunchAuthorizationRecord {
                authorization_format_version: MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION,
                server_id: config.id,
                launch_spec_digest: launch_spec_digest.clone(),
                file_identity_digest,
                authored_config_epoch: config_epoch,
                authored_config_digest: config_digest.clone(),
                authorization_policy_version: policy_version,
                authorized_at_ms,
            })
        } else {
            authorization.and_then(|mut authorization| {
                if authorization.server_id != config.id
                    || authorization.launch_spec_digest != launch_spec_digest
                    || authorization.authorization_format_version
                        != MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION
                    || authorization.authorization_policy_version
                        != MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION
                {
                    return None;
                }
                authorization.authored_config_epoch = config_epoch;
                authorization.authored_config_digest = config_digest.clone();
                Some(authorization)
            })
        };
    if config.enabled && (config.trust != McpTrustLevel::UserApproved || authorization.is_none()) {
        return Err(McpRegistryPersistenceError::AuthorizationRequired);
    }
    if config.trust == McpTrustLevel::UserApproved && authorization.is_none() {
        return Err(McpRegistryPersistenceError::AuthorizationRequired);
    }
    let now = now_ms()?.max(existing.created_at_ms);
    let record = McpPersistedRegistryRecord {
        entry: McpRegistryEntry {
            config,
            model_namespace: existing.entry.model_namespace.clone(),
            config_digest,
            config_epoch,
            revision,
        },
        source: existing.source,
        launch_spec_digest,
        launch_authorization: authorization,
        safe_error_code: None,
        created_at_ms: existing.created_at_ms,
        updated_at_ms: now,
    };
    update_row(
        transaction,
        &record.entry.config,
        &record.entry.config_digest,
        record.entry.config_epoch,
        record.entry.revision,
        &record.launch_spec_digest,
        record.launch_authorization.as_ref(),
        None,
        record.created_at_ms,
        record.updated_at_ms,
    )?;
    Ok(record)
}

fn remove_record(
    transaction: &Transaction<'_>,
    existing: McpPersistedRegistryRecord,
) -> Result<(McpRegistryEntry, McpRegistryChange), McpRegistryPersistenceError> {
    let revision = next_revision(transaction)?;
    transaction
        .execute(
            "DELETE FROM mcp_registry_model_namespaces WHERE server_id = ?1",
            [existing.entry.config.id.to_string()],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let affected = transaction
        .execute(
            "DELETE FROM mcp_registry_servers
             WHERE server_id = ?1 AND registry_revision = ?2",
            params![
                existing.entry.config.id.to_string(),
                sqlite_revision(existing.entry.revision)?
            ],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if affected != 1 {
        return Err(McpRegistryPersistenceError::Conflict);
    }
    let mut entry = existing.entry;
    entry.revision = revision;
    let change = registry_change(McpRegistryChangeKind::Removed, &entry);
    Ok((entry, change))
}

fn ensure_precondition(
    entry: &McpRegistryEntry,
    precondition: &McpRegistryMutationPrecondition,
) -> Result<(), McpRegistryPersistenceError> {
    if entry.config.id != precondition.server_id
        || entry.revision != precondition.expected_revision
        || entry.config_epoch != precondition.expected_config_epoch
        || entry.config_digest != precondition.expected_config_digest
    {
        Err(McpRegistryPersistenceError::Conflict)
    } else {
        Ok(())
    }
}
