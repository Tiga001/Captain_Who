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

const REGISTRY_METADATA_COLUMNS: &[&str] = &[
    "singleton",
    "schema_version",
    "revision_watermark",
    "updated_at",
];

const REGISTRY_SERVER_COLUMNS: &[&str] = &[
    "schema_version",
    "server_id",
    "display_name",
    "scope_kind",
    "source_kind",
    "transport_kind",
    "executable",
    "arguments_json",
    "cwd",
    "enabled",
    "trust",
    "approval_mode",
    "connect_timeout_ms",
    "request_timeout_ms",
    "shutdown_timeout_ms",
    "config_digest",
    "config_epoch",
    "registry_revision",
    "launch_spec_digest",
    "authorized_launch_spec_digest",
    "authorized_config_epoch",
    "authorized_config_digest",
    "authorization_format_version",
    "authorization_policy_version",
    "authorized_at",
    "record_state",
    "safe_error_code",
    "created_at",
    "updated_at",
];

const MODEL_NAMESPACE_COLUMNS: &[&str] = &[
    "schema_version",
    "server_id",
    "model_namespace",
    "created_at",
];

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
        run_migrations(&mut connection)?;
        reconcile_model_namespaces(&mut connection)?;
        reconcile_startup(&mut connection)?;
        reconcile_model_namespaces(&mut connection)?;
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
            let record = insert_record(&transaction, config)?;
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

    #[allow(dead_code)] // Reserved for an explicit future deauthorize management operation.
    pub(crate) fn revoke_launch_authorization(
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
            if existing.launch_authorization.is_none()
                && existing.entry.config.trust == McpTrustLevel::Untrusted
                && !existing.entry.config.enabled
            {
                transaction
                    .commit()
                    .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
                (existing.entry, None)
            } else {
                let mut config = existing.entry.config.clone();
                config.trust = McpTrustLevel::Untrusted;
                config.enabled = false;
                let record = commit_updated_record(&transaction, &existing, config, None, None)?;
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
        self.add_persisted(config)
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
                    let record = insert_record(&transaction, config)?;
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

fn run_migrations(connection: &mut Connection) -> Result<(), McpRegistryPersistenceError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    quarantine_incompatible_registry_schema(&transaction)?;
    let migrate_approval_mode_constraint =
        prepare_approval_mode_constraint_migration(&transaction)?;
    transaction
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS mcp_registry_metadata (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                schema_version INTEGER NOT NULL CHECK (schema_version = 1),
                revision_watermark INTEGER NOT NULL CHECK (
                    revision_watermark >= 0
                ),
                updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
            );

            CREATE TABLE IF NOT EXISTS mcp_registry_servers (
                schema_version INTEGER NOT NULL CHECK (schema_version = 1),
                server_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                scope_kind TEXT NOT NULL CHECK (scope_kind = 'user'),
                source_kind TEXT NOT NULL CHECK (source_kind = 'user_manual'),
                transport_kind TEXT NOT NULL CHECK (transport_kind = 'stdio'),
                executable TEXT NOT NULL,
                arguments_json TEXT NOT NULL CHECK (
                    json_valid(arguments_json)
                    AND json_type(arguments_json) = 'array'
                ),
                cwd TEXT NOT NULL,
                enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
                trust TEXT NOT NULL CHECK (
                    trust IN ('untrusted', 'user_approved')
                ),
                approval_mode TEXT NOT NULL CHECK (
                    approval_mode IN ('prompt', 'auto', 'deny')
                ),
                connect_timeout_ms INTEGER NOT NULL CHECK (
                    connect_timeout_ms BETWEEN 1 AND 10000
                ),
                request_timeout_ms INTEGER NOT NULL CHECK (
                    request_timeout_ms BETWEEN 1 AND 300000
                ),
                shutdown_timeout_ms INTEGER NOT NULL CHECK (
                    shutdown_timeout_ms BETWEEN 1 AND 2000
                ),
                config_digest TEXT NOT NULL,
                config_epoch TEXT NOT NULL,
                registry_revision INTEGER NOT NULL CHECK (
                    registry_revision > 0
                ),
                launch_spec_digest TEXT NOT NULL,
                authorized_launch_spec_digest TEXT,
                authorized_config_epoch TEXT,
                authorized_config_digest TEXT,
                authorization_format_version INTEGER,
                authorization_policy_version INTEGER,
                authorized_at INTEGER,
                record_state TEXT NOT NULL DEFAULT 'active' CHECK (
                    record_state IN ('active', 'invalid')
                ),
                safe_error_code TEXT,
                created_at INTEGER NOT NULL CHECK (created_at >= 0),
                updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
                CHECK (
                    (
                        authorized_launch_spec_digest IS NULL
                        AND authorized_config_epoch IS NULL
                        AND authorized_config_digest IS NULL
                        AND authorization_format_version IS NULL
                        AND authorization_policy_version IS NULL
                        AND authorized_at IS NULL
                    )
                    OR
                    (
                        authorized_launch_spec_digest IS NOT NULL
                        AND authorized_config_epoch IS NOT NULL
                        AND authorized_config_digest IS NOT NULL
                        AND authorization_format_version > 0
                        AND authorization_policy_version > 0
                        AND authorized_at >= 0
                    )
                )
            );

            CREATE INDEX IF NOT EXISTS mcp_registry_servers_revision
            ON mcp_registry_servers(registry_revision, server_id);

            CREATE TABLE IF NOT EXISTS mcp_registry_model_namespaces (
                schema_version INTEGER NOT NULL CHECK (schema_version = 1),
                server_id TEXT PRIMARY KEY,
                model_namespace TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL CHECK (created_at >= 0)
            );
            ",
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if migrate_approval_mode_constraint {
        finish_approval_mode_constraint_migration(&transaction)?;
    }

    let now = now_ms()?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO mcp_registry_metadata (
                 singleton, schema_version, revision_watermark, updated_at
             ) VALUES (?1, ?2, 0, ?3)",
            params![REGISTRY_METADATA_SINGLETON, REGISTRY_SCHEMA_VERSION, now],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let schema_version = transaction
        .query_row(
            "SELECT schema_version FROM mcp_registry_metadata WHERE singleton = ?1",
            [REGISTRY_METADATA_SINGLETON],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if schema_version != REGISTRY_SCHEMA_VERSION {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    transaction
        .commit()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

const APPROVAL_MODE_MIGRATION_BACKUP: &str = "mcp_registry_servers_approval_mode_prompt_deny_v1";

/// SQLite cannot alter a CHECK constraint in place. Preserve every v1 row and
/// identity field in a transaction, recreate the table with the additive
/// `auto` value, then copy the rows back verbatim.
fn prepare_approval_mode_constraint_migration(
    transaction: &Transaction<'_>,
) -> Result<bool, McpRegistryPersistenceError> {
    let Some(columns) = registry_table_columns(transaction, "mcp_registry_servers")? else {
        return Ok(false);
    };
    if !column_names_match(&columns, REGISTRY_SERVER_COLUMNS) {
        return Ok(false);
    }
    let create_sql = transaction
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'mcp_registry_servers'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if create_sql.contains("'auto'") {
        return Ok(false);
    }
    let invalid_mode_count = transaction
        .query_row(
            "SELECT COUNT(*) FROM mcp_registry_servers
             WHERE approval_mode NOT IN ('prompt', 'deny')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if invalid_mode_count != 0
        || registry_table_columns(transaction, APPROVAL_MODE_MIGRATION_BACKUP)?.is_some()
    {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    transaction
        .execute_batch(
            "ALTER TABLE mcp_registry_servers
                 RENAME TO mcp_registry_servers_approval_mode_prompt_deny_v1;
             DROP INDEX IF EXISTS mcp_registry_servers_revision;",
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    Ok(true)
}

fn finish_approval_mode_constraint_migration(
    transaction: &Transaction<'_>,
) -> Result<(), McpRegistryPersistenceError> {
    let columns = REGISTRY_SERVER_COLUMNS.join(", ");
    let copy = format!(
        "INSERT INTO mcp_registry_servers ({columns})
         SELECT {columns}
         FROM {APPROVAL_MODE_MIGRATION_BACKUP};
         DROP TABLE {APPROVAL_MODE_MIGRATION_BACKUP};"
    );
    transaction
        .execute_batch(&copy)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

/// Repairs the additive Host-owned model namespace index without changing any
/// Registry configuration identity, revision, or launch authorization.
///
/// Existing valid mappings are immutable. Missing or malformed side-table
/// rows are rebuilt deterministically from the current persisted display name;
/// stale rows for removed/quarantined servers are deleted. The caller runs
/// this once before startup record decoding and once after reconciliation.
fn reconcile_model_namespaces(
    connection: &mut Connection,
) -> Result<(), McpRegistryPersistenceError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;

    let candidates = {
        let mut statement = transaction
            .prepare(
                "SELECT server_id, display_name, created_at
                 FROM mcp_registry_servers
                 WHERE record_state = 'active'
                 ORDER BY created_at, server_id",
            )
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let rows = statement
            .query_map([], |row| {
                let server_id = match row.get_ref(0)? {
                    ValueRef::Text(value) => std::str::from_utf8(value).ok().map(str::to_string),
                    _ => None,
                };
                let display_name = match row.get_ref(1)? {
                    ValueRef::Text(value) => std::str::from_utf8(value).ok().map(str::to_string),
                    _ => None,
                };
                let created_at = match row.get_ref(2)? {
                    ValueRef::Integer(value) => Some(value),
                    _ => None,
                };
                Ok(server_id.zip(display_name).zip(created_at).map(
                    |((server_id, display_name), created_at)| (server_id, display_name, created_at),
                ))
            })
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        rows.into_iter().flatten().collect::<Vec<_>>()
    };
    let candidates = candidates
        .into_iter()
        .filter_map(|(stored_id, display_name, created_at)| {
            McpServerId::from_str(&stored_id)
                .ok()
                .map(|server_id| (stored_id, server_id, display_name, created_at))
        })
        .collect::<Vec<_>>();
    let active_ids = candidates
        .iter()
        .map(|(stored_id, _, _, _)| stored_id.clone())
        .collect::<std::collections::BTreeSet<_>>();

    let persisted = {
        let mut statement = transaction
            .prepare(
                "SELECT rowid, schema_version, server_id, model_namespace
                 FROM mcp_registry_model_namespaces
                 ORDER BY rowid",
            )
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    match row.get_ref(1)? {
                        ValueRef::Integer(value) => Some(value),
                        _ => None,
                    },
                    match row.get_ref(2)? {
                        ValueRef::Text(value) => {
                            std::str::from_utf8(value).ok().map(str::to_string)
                        }
                        _ => None,
                    },
                    match row.get_ref(3)? {
                        ValueRef::Text(value) => {
                            std::str::from_utf8(value).ok().map(str::to_string)
                        }
                        _ => None,
                    },
                ))
            })
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        rows
    };

    let mut mapped_ids = std::collections::BTreeSet::new();
    let mut occupied = std::collections::BTreeSet::<McpModelNamespace>::new();
    for (rowid, schema_version, server_id, raw_namespace) in persisted {
        let valid_mapping = schema_version
            .filter(|version| *version == MODEL_NAMESPACE_SCHEMA_VERSION)
            .zip(server_id)
            .zip(raw_namespace)
            .and_then(|((_, server_id), raw_namespace)| {
                McpModelNamespace::from_str(&raw_namespace)
                    .ok()
                    .map(|namespace| (server_id, namespace))
            })
            .filter(|(server_id, _)| active_ids.contains(server_id))
            .filter(|(server_id, namespace)| {
                !mapped_ids.contains(server_id) && !occupied.contains(namespace)
            });
        if let Some((server_id, namespace)) = valid_mapping {
            mapped_ids.insert(server_id);
            occupied.insert(namespace);
        } else {
            transaction
                .execute(
                    "DELETE FROM mcp_registry_model_namespaces WHERE rowid = ?1",
                    [rowid],
                )
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        }
    }

    for (stored_id, server_id, display_name, created_at) in candidates {
        if mapped_ids.contains(&stored_id) {
            continue;
        }
        let namespace = allocate_model_namespace(&display_name, server_id, occupied.iter())
            .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?;
        transaction
            .execute(
                "INSERT INTO mcp_registry_model_namespaces (
                     schema_version, server_id, model_namespace, created_at
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    MODEL_NAMESPACE_SCHEMA_VERSION,
                    stored_id,
                    namespace.as_str(),
                    created_at.max(0),
                ],
            )
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        occupied.insert(namespace);
    }

    transaction
        .commit()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

fn quarantine_incompatible_registry_schema(
    transaction: &Transaction<'_>,
) -> Result<(), McpRegistryPersistenceError> {
    let namespace_columns = registry_table_columns(transaction, "mcp_registry_model_namespaces")?;
    let namespace_shape_is_incompatible = match namespace_columns.as_deref() {
        Some(columns) => {
            !column_names_match(columns, MODEL_NAMESPACE_COLUMNS)
                || !model_namespace_table_constraints_are_compatible(transaction)?
        }
        None => false,
    };
    if namespace_shape_is_incompatible {
        quarantine_registry_table(transaction, "mcp_registry_model_namespaces")?;
    }

    let metadata_columns = registry_table_columns(transaction, "mcp_registry_metadata")?;
    let metadata_shape_is_incompatible = metadata_columns
        .as_deref()
        .is_some_and(|columns| !column_names_match(columns, REGISTRY_METADATA_COLUMNS));

    if metadata_shape_is_incompatible {
        quarantine_registry_table(transaction, "mcp_registry_metadata")?;
        quarantine_registry_table(transaction, "mcp_registry_servers")?;
        return Ok(());
    }

    if metadata_columns.is_some() && !registry_metadata_version_is_compatible(transaction)? {
        quarantine_registry_table(transaction, "mcp_registry_metadata")?;
        quarantine_registry_table(transaction, "mcp_registry_servers")?;
        return Ok(());
    }

    let server_columns = registry_table_columns(transaction, "mcp_registry_servers")?;
    if server_columns
        .as_deref()
        .is_some_and(|columns| !column_names_match(columns, REGISTRY_SERVER_COLUMNS))
    {
        quarantine_registry_table(transaction, "mcp_registry_servers")?;
    }
    Ok(())
}

fn model_namespace_table_constraints_are_compatible(
    transaction: &Transaction<'_>,
) -> Result<bool, McpRegistryPersistenceError> {
    if transaction
        .prepare("SELECT rowid FROM mcp_registry_model_namespaces LIMIT 0")
        .is_err()
    {
        return Ok(false);
    }

    let table_info_sql = format!(
        "PRAGMA table_info({})",
        quote_identifier("mcp_registry_model_namespaces")
    );
    let primary_key_columns = {
        let mut statement = transaction
            .prepare(&table_info_sql)
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let mut rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        rows.retain(|(_, primary_key_position)| *primary_key_position > 0);
        rows.sort_by_key(|(_, primary_key_position)| *primary_key_position);
        rows.into_iter().map(|(name, _)| name).collect::<Vec<_>>()
    };
    if primary_key_columns != ["server_id"] {
        return Ok(false);
    }

    let index_list_sql = format!(
        "PRAGMA index_list({})",
        quote_identifier("mcp_registry_model_namespaces")
    );
    let unique_indexes = {
        let mut statement = transaction
            .prepare(&index_list_sql)
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, i64>(4)? != 0,
                ))
            })
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        rows
    };
    for (index_name, unique, partial) in unique_indexes {
        if !unique || partial {
            continue;
        }
        let index_info_sql = format!("PRAGMA index_info({})", quote_identifier(&index_name));
        let columns = {
            let mut statement = transaction
                .prepare(&index_info_sql)
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            let rows = statement
                .query_map([], |row| row.get::<_, Option<String>>(2))
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            rows
        };
        if columns == [Some("model_namespace".to_string())] {
            return Ok(true);
        }
    }
    Ok(false)
}

fn registry_metadata_version_is_compatible(
    transaction: &Transaction<'_>,
) -> Result<bool, McpRegistryPersistenceError> {
    let row_count = transaction
        .query_row("SELECT COUNT(*) FROM mcp_registry_metadata", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if row_count == 0 {
        return Ok(true);
    }
    if row_count != 1 {
        return Ok(false);
    }
    let identity = transaction.query_row(
        "SELECT singleton, schema_version FROM mcp_registry_metadata LIMIT 1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );
    Ok(matches!(
        identity,
        Ok((REGISTRY_METADATA_SINGLETON, REGISTRY_SCHEMA_VERSION))
    ))
}

fn registry_table_columns(
    transaction: &Transaction<'_>,
    table_name: &str,
) -> Result<Option<Vec<String>>, McpRegistryPersistenceError> {
    let object_type = transaction
        .query_row(
            "SELECT type FROM sqlite_schema WHERE name = ?1",
            [table_name],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let Some(object_type) = object_type else {
        return Ok(None);
    };
    if object_type != "table" {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    let sql = format!("PRAGMA table_info({})", quote_identifier(table_name));
    let mut statement = transaction
        .prepare(&sql)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    Ok(Some(columns))
}

fn column_names_match(actual: &[String], expected: &[&str]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
}

fn quarantine_registry_table(
    transaction: &Transaction<'_>,
    table_name: &str,
) -> Result<(), McpRegistryPersistenceError> {
    if registry_table_columns(transaction, table_name)?.is_none() {
        return Ok(());
    }
    let quarantine_name = next_quarantine_table_name(transaction, table_name)?;
    let rename = format!(
        "ALTER TABLE {} RENAME TO {}",
        quote_identifier(table_name),
        quote_identifier(&quarantine_name)
    );
    transaction
        .execute_batch(&rename)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;

    if table_name == "mcp_registry_servers" {
        let revision_index_owner = transaction
            .query_row(
                "SELECT tbl_name FROM sqlite_schema
                 WHERE type = 'index' AND name = 'mcp_registry_servers_revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        match revision_index_owner.as_deref() {
            None => {}
            Some(owner) if owner == quarantine_name => transaction
                .execute_batch("DROP INDEX mcp_registry_servers_revision")
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?,
            Some(_) => return Err(McpRegistryPersistenceError::CorruptRecord),
        }
    }
    Ok(())
}

fn next_quarantine_table_name(
    transaction: &Transaction<'_>,
    table_name: &str,
) -> Result<String, McpRegistryPersistenceError> {
    for sequence in 1_u32..=10_000 {
        let candidate = format!("{table_name}_incompatible_v{REGISTRY_SCHEMA_VERSION}_{sequence}");
        let exists = transaction
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = ?1",
                [&candidate],
                |_| Ok(()),
            )
            .optional()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .is_some();
        if !exists {
            return Ok(candidate);
        }
    }
    Err(McpRegistryPersistenceError::StorageUnavailable)
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

struct StartupLaunchAuthorizationValidation {
    server_id: McpServerId,
    revision: u64,
    config_epoch: McpConfigEpoch,
    config_digest: McpConfigDigest,
    launch_spec_digest: McpLaunchSpecDigest,
    valid: bool,
}

impl StartupLaunchAuthorizationValidation {
    fn matches(&self, record: &McpPersistedRegistryRecord) -> bool {
        self.server_id == record.entry.config.id
            && self.revision == record.entry.revision
            && self.config_epoch == record.entry.config_epoch
            && self.config_digest == record.entry.config_digest
            && self.launch_spec_digest == record.launch_spec_digest
    }
}

fn inspect_startup_launch_authorizations(
    connection: &mut Connection,
) -> Result<BTreeMap<i64, StartupLaunchAuthorizationValidation>, McpRegistryPersistenceError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let candidates = scan_raw_records(&transaction, false)?
        .into_iter()
        .filter_map(|scanned| {
            let raw = scanned.record?;
            if scanned.already_invalid
                || raw.record_state == RECORD_STATE_INVALID
                || raw.schema_version != REGISTRY_SCHEMA_VERSION
            {
                return None;
            }
            let record = decode_record(raw).ok()?;
            (record.entry.config.enabled
                || record.entry.config.trust == McpTrustLevel::UserApproved)
                .then_some((scanned.row_id, record))
        })
        .collect::<Vec<_>>();
    transaction
        .commit()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;

    Ok(candidates
        .into_iter()
        .map(|(row_id, record)| {
            let validation = StartupLaunchAuthorizationValidation {
                server_id: record.entry.config.id,
                revision: record.entry.revision,
                config_epoch: record.entry.config_epoch,
                config_digest: record.entry.config_digest.clone(),
                launch_spec_digest: record.launch_spec_digest.clone(),
                valid: launch_authorization_is_valid(&record),
            };
            (row_id, validation)
        })
        .collect())
}

fn reconcile_startup(
    connection: &mut Connection,
) -> Result<McpStartupReconciliationReport, McpRegistryPersistenceError> {
    // Filesystem identity validation may touch slow or unavailable mounts.
    // Perform that bounded I/O outside the short write transaction, then bind
    // each result back to the exact row identity before trusting it.
    let launch_authorizations = inspect_startup_launch_authorizations(connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let scanned_records = scan_raw_records(&transaction, false)?;
    let max_record_revision = scanned_records
        .iter()
        .filter_map(|record| record.revision_candidate)
        .max()
        .unwrap_or(0);
    let mut revision = read_revision(&transaction)?;
    if max_record_revision > revision {
        revision = max_record_revision;
        write_revision(&transaction, revision)?;
    }

    let mut report = McpStartupReconciliationReport {
        recovered_records: 0,
        quarantined_records: 0,
        revision,
    };
    for scanned in scanned_records {
        if scanned.already_invalid {
            continue;
        }
        let Some(raw) = scanned.record else {
            let revision = next_revision(&transaction)?;
            quarantine_record_by_rowid(&transaction, scanned.row_id, revision)?;
            report.quarantined_records += 1;
            report.revision = revision;
            continue;
        };
        if raw.record_state == RECORD_STATE_INVALID {
            continue;
        }
        if raw.schema_version != REGISTRY_SCHEMA_VERSION {
            let next = next_revision(&transaction)?;
            quarantine_record_by_rowid(&transaction, scanned.row_id, next)?;
            report.quarantined_records += 1;
            report.revision = next;
            continue;
        }
        if let Ok(record) = decode_record(raw.clone()) {
            let requires_live_authorization = record.entry.config.enabled
                || record.entry.config.trust == McpTrustLevel::UserApproved;
            let live_authorization_valid = launch_authorizations
                .get(&scanned.row_id)
                .is_some_and(|validation| validation.matches(&record) && validation.valid);
            if requires_live_authorization && !live_authorization_valid {
                // Continue through the fail-closed recovery path below. A
                // replaced executable or script must never remain enabled
                // across Host restart.
            } else {
                continue;
            }
        }
        match recover_config(&raw) {
            Ok(config) => {
                let existing_created_at = raw.created_at.max(0);
                let now = now_ms()?.max(existing_created_at);
                let config_epoch = McpConfigEpoch::new();
                let config_digest = config_digest(&config)
                    .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?;
                let launch_spec_digest = compute_launch_spec_digest(&config)?;
                let revision = next_revision(&transaction)?;
                update_row(
                    &transaction,
                    &config,
                    &config_digest,
                    config_epoch,
                    revision,
                    &launch_spec_digest,
                    None,
                    Some(SAFE_ERROR_RECONCILED),
                    existing_created_at,
                    now,
                )?;
                report.recovered_records += 1;
                report.revision = revision;
            }
            Err(_) => {
                let revision = next_revision(&transaction)?;
                quarantine_record_by_rowid(&transaction, scanned.row_id, revision)?;
                report.quarantined_records += 1;
                report.revision = revision;
            }
        }
    }
    transaction
        .commit()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    Ok(report)
}

fn recover_config(raw: &RawRegistryRecord) -> Result<McpServerConfig, McpRegistryPersistenceError> {
    let mut config = config_from_raw(raw)?;
    config.enabled = false;
    config.trust = McpTrustLevel::Untrusted;
    normalize_config(config)
}

fn insert_record(
    transaction: &Transaction<'_>,
    config: McpServerConfig,
) -> Result<McpPersistedRegistryRecord, McpRegistryPersistenceError> {
    ensure_registry_capacity(transaction)?;
    let occupied_namespaces = load_model_namespaces(transaction)?;
    let model_namespace =
        allocate_model_namespace(&config.display_name, config.id, occupied_namespaces.iter())
            .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
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

pub(crate) fn launch_authorization_is_valid(record: &McpPersistedRegistryRecord) -> bool {
    launch_authorization_identity_is_valid(record)
        && record
            .launch_authorization
            .as_ref()
            .is_some_and(|authorization| {
                compute_launch_file_identity_digest(
                    &record.entry.config,
                    &record.launch_spec_digest,
                )
                .is_ok_and(|digest| digest == authorization.file_identity_digest)
            })
}

pub(crate) fn launch_authorization_identity_is_valid(record: &McpPersistedRegistryRecord) -> bool {
    record.entry.config.trust == McpTrustLevel::UserApproved
        && record
            .launch_authorization
            .as_ref()
            .is_some_and(|authorization| {
                authorization.server_id == record.entry.config.id
                    && authorization.launch_spec_digest == record.launch_spec_digest
                    && authorization.authored_config_epoch == record.entry.config_epoch
                    && authorization.authored_config_digest == record.entry.config_digest
                    && authorization.authorization_format_version
                        == MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION
                    && authorization.authorization_policy_version
                        == MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION
            })
}

fn normalize_new_config(
    config: McpServerConfig,
) -> Result<McpServerConfig, McpRegistryPersistenceError> {
    let config = normalize_config(config)?;
    if config.enabled || config.trust != McpTrustLevel::Untrusted {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    Ok(config)
}

fn normalize_config(
    mut config: McpServerConfig,
) -> Result<McpServerConfig, McpRegistryPersistenceError> {
    if config.scope != McpServerScope::User
        || config.display_name.trim().is_empty()
        || config.display_name.len() > MAX_DISPLAY_NAME_BYTES
        || config
            .display_name
            .chars()
            .any(is_unsafe_display_name_character)
    {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    if !matches!(
        config.trust,
        McpTrustLevel::Untrusted | McpTrustLevel::UserApproved
    ) || !(1..=MAX_CONNECT_TIMEOUT_MS).contains(&config.connect_timeout_ms)
        || !(1..=MAX_REQUEST_TIMEOUT_MS).contains(&config.request_timeout_ms)
        || !(1..=MAX_SHUTDOWN_TIMEOUT_MS).contains(&config.shutdown_timeout_ms)
    {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    let McpTransportConfig::Stdio(stdio) = &mut config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    if !stdio.environment.is_empty()
        || stdio.arguments.len() > MAX_ARGUMENTS
        || stdio
            .arguments
            .iter()
            .any(|argument| argument.len() > MAX_ARGUMENT_BYTES || argument.contains('\0'))
        || stdio
            .arguments
            .iter()
            .map(String::len)
            .try_fold(0usize, usize::checked_add)
            .is_none_or(|total| total > MAX_ARGUMENTS_TOTAL_BYTES)
    {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    stdio.program = normalize_absolute_path(&stdio.program)?;
    stdio.cwd = normalize_absolute_path(&stdio.cwd)?;
    if path_text(&stdio.program)?.len() > MAX_PATH_BYTES
        || path_text(&stdio.cwd)?.len() > MAX_PATH_BYTES
    {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    Ok(config)
}

fn is_unsafe_display_name_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{0085}'
                | '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
        )
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, McpRegistryPersistenceError> {
    if !path.is_absolute() {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() || normalized.as_os_str().is_empty() {
                    return Err(McpRegistryPersistenceError::InvalidConfig);
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    if !normalized.is_absolute() {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    path_text(&normalized)?;
    Ok(normalized)
}

fn path_text(path: &Path) -> Result<&str, McpRegistryPersistenceError> {
    path.to_str()
        .filter(|value| !value.contains('\0'))
        .ok_or(McpRegistryPersistenceError::InvalidConfig)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchSpec<'a> {
    authorization_format_version: u32,
    server_id: String,
    scope: &'static str,
    source: &'static str,
    transport: &'static str,
    executable: &'a str,
    arguments: &'a [String],
    cwd: &'a str,
    environment: &'a [String],
}

pub(crate) fn compute_launch_spec_digest(
    config: &McpServerConfig,
) -> Result<McpLaunchSpecDigest, McpRegistryPersistenceError> {
    let config = normalize_config(config.clone())?;
    let McpTransportConfig::Stdio(stdio) = &config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    if !stdio.environment.is_empty() || config.scope != McpServerScope::User {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    let empty_environment: &[String] = &[];
    let launch = LaunchSpec {
        authorization_format_version: MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION,
        server_id: config.id.to_string(),
        scope: "user",
        source: SOURCE_USER_MANUAL,
        transport: "stdio",
        executable: path_text(&stdio.program)?,
        arguments: &stdio.arguments,
        cwd: path_text(&stdio.cwd)?,
        environment: empty_environment,
    };
    let encoded =
        serde_json::to_vec(&launch).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let mut hasher = Sha256::new();
    hasher.update(LAUNCH_DIGEST_DOMAIN);
    hasher.update(encoded);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(64);
    use fmt::Write as _;
    for byte in digest {
        write!(output, "{byte:02x}")
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    }
    Ok(McpLaunchSpecDigest(output))
}

pub(crate) fn compute_launch_file_identity_digest(
    config: &McpServerConfig,
    launch_spec_digest: &McpLaunchSpecDigest,
) -> Result<McpLaunchSpecDigest, McpRegistryPersistenceError> {
    prepare_launch_file_identity(config, launch_spec_digest).map(|(digest, _)| digest)
}

pub(crate) fn prepare_launch_file_identity(
    config: &McpServerConfig,
    launch_spec_digest: &McpLaunchSpecDigest,
) -> Result<(McpLaunchSpecDigest, McpServerConfig), McpRegistryPersistenceError> {
    let config = normalize_config(config.clone())?;
    let McpTransportConfig::Stdio(stdio) = &config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    // Keep the normalized logical executable path for process creation. In
    // particular, Python virtual environments intentionally expose `bin/python`
    // as a symlink; replacing it with the canonical base interpreter changes
    // Python's environment discovery and loses the venv site-packages. The
    // identity below still binds both this logical path and its canonical
    // target before the authorized connector reaches spawn.
    let launch_program = stdio.program.clone();

    let mut hasher = Sha256::new();
    hasher.update(LAUNCH_FILE_IDENTITY_DOMAIN);
    hash_identity_field(&mut hasher, launch_spec_digest.as_str().as_bytes());
    let mut remaining_content_hash_bytes = MAX_LAUNCH_CONTENT_HASH_TOTAL_BYTES;
    hash_required_launch_path(
        &mut hasher,
        b"executable",
        &stdio.program,
        true,
        &mut remaining_content_hash_bytes,
    )?;
    let canonical_cwd = hash_required_launch_path(
        &mut hasher,
        b"cwd",
        &stdio.cwd,
        false,
        &mut remaining_content_hash_bytes,
    )?;

    let mut code_inputs = 0_usize;
    let mut canonical_arguments = stdio.arguments.clone();
    for (index, argument) in stdio.arguments.iter().enumerate() {
        let Some(code_input) = launch_code_input(argument) else {
            continue;
        };
        code_inputs = code_inputs
            .checked_add(1)
            .filter(|count| *count <= MAX_LAUNCH_CODE_INPUTS)
            .ok_or(McpRegistryPersistenceError::InvalidConfig)?;
        hash_identity_field(&mut hasher, b"code-input");
        hasher.update((index as u64).to_le_bytes());
        let candidate = if code_input.path.is_absolute() {
            code_input.path.to_path_buf()
        } else {
            stdio.cwd.join(code_input.path)
        };
        let canonical = hash_required_launch_path(
            &mut hasher,
            b"code-input-path",
            &candidate,
            true,
            &mut remaining_content_hash_bytes,
        )?;
        let canonical = path_text(&canonical)?;
        canonical_arguments[index] = match code_input.inline_prefix {
            Some(prefix) => format!("{prefix}{canonical}"),
            None => canonical.to_string(),
        };
    }

    let digest = hasher.finalize();
    let mut output = String::with_capacity(64);
    use fmt::Write as _;
    for byte in digest {
        write!(output, "{byte:02x}")
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    }
    let mut launch_config = config;
    let McpTransportConfig::Stdio(stdio) = &mut launch_config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    stdio.program = launch_program;
    stdio.cwd = canonical_cwd;
    stdio.arguments = canonical_arguments;
    Ok((McpLaunchSpecDigest(output), launch_config))
}

fn hash_required_launch_path(
    hasher: &mut Sha256,
    role: &[u8],
    path: &Path,
    require_file: bool,
    remaining_content_hash_bytes: &mut u64,
) -> Result<PathBuf, McpRegistryPersistenceError> {
    hash_identity_field(hasher, role);
    hash_identity_field(hasher, path_text(path)?.as_bytes());
    let canonical =
        std::fs::canonicalize(path).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let metadata =
        std::fs::metadata(&canonical).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    if (require_file && !metadata.is_file()) || (!require_file && !metadata.is_dir()) {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    hash_canonical_launch_object(hasher, &canonical, metadata, remaining_content_hash_bytes)?;
    Ok(canonical)
}

#[derive(Clone, Copy)]
struct LaunchCodeInput<'a> {
    path: &'a Path,
    inline_prefix: Option<&'static str>,
}

fn launch_code_input(argument: &str) -> Option<LaunchCodeInput<'_>> {
    const INLINE_CODE_PATH_FLAGS: [&str; 5] = [
        "--require=",
        "--import=",
        "--loader=",
        "--experimental-loader=",
        "--module=",
    ];
    if let Some((prefix, value)) = INLINE_CODE_PATH_FLAGS
        .iter()
        .find_map(|prefix| argument.strip_prefix(prefix).map(|value| (*prefix, value)))
    {
        let path = Path::new(value);
        let explicit_filesystem_path = path.is_absolute()
            || matches!(
                path.components().next(),
                Some(Component::CurDir | Component::ParentDir)
            );
        if !explicit_filesystem_path {
            // URL imports and package specifiers are resolved by the runtime;
            // guessing them as cwd-relative files would reject valid launches.
            return None;
        }
        return has_code_extension(path).then_some(LaunchCodeInput {
            path,
            inline_prefix: Some(prefix),
        });
    }
    let path = Path::new(argument);
    if argument.is_empty()
        || argument.starts_with('-')
        || (!path.is_absolute() && has_uri_scheme(argument))
    {
        return None;
    }
    has_code_extension(path).then_some(LaunchCodeInput {
        path,
        inline_prefix: None,
    })
}

fn has_uri_scheme(value: &str) -> bool {
    let Some(separator) = value.find(':') else {
        return false;
    };
    let scheme = &value[..separator];
    !scheme.is_empty()
        && scheme
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn has_code_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .is_some_and(|extension| {
            matches!(
                extension.as_str(),
                "js" | "mjs"
                    | "cjs"
                    | "ts"
                    | "mts"
                    | "cts"
                    | "py"
                    | "pyw"
                    | "rb"
                    | "pl"
                    | "php"
                    | "sh"
                    | "bash"
                    | "zsh"
                    | "fish"
                    | "ps1"
                    | "bat"
                    | "cmd"
                    | "jar"
                    | "wasm"
            )
        })
}

fn hash_canonical_launch_object(
    hasher: &mut Sha256,
    canonical: &Path,
    metadata_before: std::fs::Metadata,
    remaining_content_hash_bytes: &mut u64,
) -> Result<(), McpRegistryPersistenceError> {
    hash_identity_field(hasher, path_text(canonical)?.as_bytes());
    let before = LaunchMetadataSnapshot::from_metadata(&metadata_before)?;
    let hash_contents = metadata_before.is_file()
        && metadata_before.len() <= MAX_LAUNCH_CONTENT_HASH_FILE_BYTES
        && metadata_before.len() <= *remaining_content_hash_bytes;
    before.hash_into(hasher, !hash_contents);

    if hash_contents {
        *remaining_content_hash_bytes -= metadata_before.len();
        hash_launch_file_contents(hasher, canonical, &before)?;
    }

    let metadata_after =
        std::fs::metadata(canonical).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    if before != LaunchMetadataSnapshot::from_metadata(&metadata_after)? {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    Ok(())
}

fn hash_launch_file_contents(
    hasher: &mut Sha256,
    canonical: &Path,
    expected: &LaunchMetadataSnapshot,
) -> Result<(), McpRegistryPersistenceError> {
    let mut file =
        std::fs::File::open(canonical).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let opened = file
        .metadata()
        .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    if LaunchMetadataSnapshot::from_metadata(&opened)? != *expected {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }

    let mut content_hasher = Sha256::new();
    let mut buffer = [0_u8; LAUNCH_IDENTITY_READ_BUFFER_BYTES];
    let mut bytes_read = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
        if count == 0 {
            break;
        }
        bytes_read = bytes_read
            .checked_add(count as u64)
            .filter(|total| *total <= expected.len)
            .ok_or(McpRegistryPersistenceError::InvalidConfig)?;
        content_hasher.update(&buffer[..count]);
    }
    if bytes_read != expected.len {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }

    let closed_over = file
        .metadata()
        .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    if LaunchMetadataSnapshot::from_metadata(&closed_over)? != *expected {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    hash_identity_field(hasher, b"content-sha256");
    hash_identity_field(hasher, content_hasher.finalize().as_slice());
    Ok(())
}

fn hash_identity_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

#[derive(PartialEq, Eq)]
struct LaunchMetadataSnapshot {
    kind: u8,
    len: u64,
    modified_nanos: Option<u128>,
    readonly: bool,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanos: i64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    owner: u32,
    #[cfg(unix)]
    group: u32,
}

impl LaunchMetadataSnapshot {
    fn from_metadata(metadata: &std::fs::Metadata) -> Result<Self, McpRegistryPersistenceError> {
        let is_file = metadata.is_file();
        let kind = if is_file {
            1
        } else if metadata.is_dir() {
            2
        } else {
            return Err(McpRegistryPersistenceError::InvalidConfig);
        };
        let modified_nanos = is_file
            .then(|| {
                metadata.modified().ok().and_then(|modified| {
                    modified
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_nanos())
                })
            })
            .flatten();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                kind,
                // Directory size, mtime, permissions, and ownership legitimately
                // change as applications create files under an authorized cwd.
                // Its canonical target plus device/inode are the stable identity.
                len: if is_file { metadata.len() } else { 0 },
                modified_nanos,
                readonly: is_file && metadata.permissions().readonly(),
                device: metadata.dev(),
                inode: metadata.ino(),
                changed_seconds: if is_file { metadata.ctime() } else { 0 },
                changed_nanos: if is_file { metadata.ctime_nsec() } else { 0 },
                mode: if is_file { metadata.mode() } else { 0 },
                owner: if is_file { metadata.uid() } else { 0 },
                group: if is_file { metadata.gid() } else { 0 },
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                kind,
                len: if is_file { metadata.len() } else { 0 },
                modified_nanos,
                readonly: is_file && metadata.permissions().readonly(),
            })
        }
    }

    fn hash_into(&self, hasher: &mut Sha256, include_ctime: bool) {
        hasher.update([self.kind]);
        hasher.update(self.len.to_le_bytes());
        match self.modified_nanos {
            Some(value) => {
                hasher.update([1]);
                hasher.update(value.to_le_bytes());
            }
            None => hasher.update([0]),
        }
        hasher.update([u8::from(self.readonly)]);
        #[cfg(unix)]
        {
            hasher.update(self.device.to_le_bytes());
            hasher.update(self.inode.to_le_bytes());
            if include_ctime {
                // Large runtime binaries keep the cheap ctime tamper signal;
                // complete content hashing is reserved for bounded files.
                hasher.update(self.changed_seconds.to_le_bytes());
                hasher.update(self.changed_nanos.to_le_bytes());
            }
            // For content-hashed files, ctime remains part of the before/after
            // equality check but not the persisted identity. macOS may update
            // it for quarantine/xattr bookkeeping without changing authority.
            hasher.update(self.mode.to_le_bytes());
            hasher.update(self.owner.to_le_bytes());
            hasher.update(self.group.to_le_bytes());
        }
    }
}

fn insert_row(
    transaction: &Transaction<'_>,
    record: &McpPersistedRegistryRecord,
) -> Result<(), McpRegistryPersistenceError> {
    let McpTransportConfig::Stdio(stdio) = &record.entry.config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    let arguments_json = serde_json::to_string(&stdio.arguments)
        .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let authorization = authorization_parts(record.launch_authorization.as_ref());
    transaction
        .execute(
            "INSERT INTO mcp_registry_servers (
                 schema_version, server_id, display_name, scope_kind, source_kind,
                 transport_kind, executable, arguments_json, cwd, enabled, trust,
                 approval_mode, connect_timeout_ms, request_timeout_ms,
                 shutdown_timeout_ms, config_digest, config_epoch,
                 registry_revision, launch_spec_digest,
                 authorized_launch_spec_digest, authorized_config_epoch,
                 authorized_config_digest, authorization_format_version,
                 authorization_policy_version, authorized_at, record_state,
                 safe_error_code, created_at, updated_at
             ) VALUES (
                 ?1, ?2, ?3, 'user', 'user_manual', 'stdio', ?4, ?5, ?6, ?7,
                 ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
                 ?20, ?21, ?22, 'active', ?23, ?24, ?25
             )",
            params![
                REGISTRY_SCHEMA_VERSION,
                record.entry.config.id.to_string(),
                record.entry.config.display_name,
                path_text(&stdio.program)?,
                arguments_json,
                path_text(&stdio.cwd)?,
                bool_integer(record.entry.config.enabled),
                trust_text(record.entry.config.trust)?,
                approval_mode_text(record.entry.config.approval_mode)?,
                sqlite_u64(record.entry.config.connect_timeout_ms)?,
                sqlite_u64(record.entry.config.request_timeout_ms)?,
                sqlite_u64(record.entry.config.shutdown_timeout_ms)?,
                record.entry.config_digest.as_str(),
                record.entry.config_epoch.to_string(),
                sqlite_revision(record.entry.revision)?,
                record.launch_spec_digest.as_str(),
                authorization.launch_digest,
                authorization.config_epoch,
                authorization.config_digest,
                authorization.format_version,
                authorization.policy_version,
                authorization.authorized_at,
                record.safe_error_code,
                record.created_at_ms,
                record.updated_at_ms,
            ],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    transaction
        .execute(
            "INSERT INTO mcp_registry_model_namespaces (
                 schema_version, server_id, model_namespace, created_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                MODEL_NAMESPACE_SCHEMA_VERSION,
                record.entry.config.id.to_string(),
                record.entry.model_namespace.as_str(),
                record.created_at_ms,
            ],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn update_row(
    transaction: &Transaction<'_>,
    config: &McpServerConfig,
    digest: &McpConfigDigest,
    epoch: McpConfigEpoch,
    revision: u64,
    launch_digest: &McpLaunchSpecDigest,
    authorization: Option<&McpLaunchAuthorizationRecord>,
    safe_error_code: Option<&str>,
    created_at_ms: i64,
    updated_at_ms: i64,
) -> Result<(), McpRegistryPersistenceError> {
    let McpTransportConfig::Stdio(stdio) = &config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    let arguments_json = serde_json::to_string(&stdio.arguments)
        .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let authorization = authorization_parts(authorization);
    let affected = transaction
        .execute(
            "UPDATE mcp_registry_servers SET
                 schema_version = ?2,
                 display_name = ?3,
                 scope_kind = 'user',
                 source_kind = 'user_manual',
                 transport_kind = 'stdio',
                 executable = ?4,
                 arguments_json = ?5,
                 cwd = ?6,
                 enabled = ?7,
                 trust = ?8,
                 approval_mode = ?9,
                 connect_timeout_ms = ?10,
                 request_timeout_ms = ?11,
                 shutdown_timeout_ms = ?12,
                 config_digest = ?13,
                 config_epoch = ?14,
                 registry_revision = ?15,
                 launch_spec_digest = ?16,
                 authorized_launch_spec_digest = ?17,
                 authorized_config_epoch = ?18,
                 authorized_config_digest = ?19,
                 authorization_format_version = ?20,
                 authorization_policy_version = ?21,
                 authorized_at = ?22,
                 record_state = 'active',
                 safe_error_code = ?23,
                 created_at = ?24,
                 updated_at = ?25
             WHERE server_id = ?1",
            params![
                config.id.to_string(),
                REGISTRY_SCHEMA_VERSION,
                config.display_name,
                path_text(&stdio.program)?,
                arguments_json,
                path_text(&stdio.cwd)?,
                bool_integer(config.enabled),
                trust_text(config.trust)?,
                approval_mode_text(config.approval_mode)?,
                sqlite_u64(config.connect_timeout_ms)?,
                sqlite_u64(config.request_timeout_ms)?,
                sqlite_u64(config.shutdown_timeout_ms)?,
                digest.as_str(),
                epoch.to_string(),
                sqlite_revision(revision)?,
                launch_digest.as_str(),
                authorization.launch_digest,
                authorization.config_epoch,
                authorization.config_digest,
                authorization.format_version,
                authorization.policy_version,
                authorization.authorized_at,
                safe_error_code,
                created_at_ms,
                updated_at_ms,
            ],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if affected != 1 {
        return Err(McpRegistryPersistenceError::NotFound);
    }
    Ok(())
}

struct AuthorizationSqlParts<'a> {
    launch_digest: Option<&'a str>,
    config_epoch: Option<String>,
    config_digest: Option<&'a str>,
    format_version: Option<i64>,
    policy_version: Option<i64>,
    authorized_at: Option<i64>,
}

fn authorization_parts(
    authorization: Option<&McpLaunchAuthorizationRecord>,
) -> AuthorizationSqlParts<'_> {
    AuthorizationSqlParts {
        launch_digest: authorization.map(|value| value.file_identity_digest.as_str()),
        config_epoch: authorization.map(|value| value.authored_config_epoch.to_string()),
        config_digest: authorization.map(|value| value.authored_config_digest.as_str()),
        format_version: authorization.map(|value| i64::from(value.authorization_format_version)),
        policy_version: authorization.map(|value| i64::from(value.authorization_policy_version)),
        authorized_at: authorization.map(|value| value.authorized_at_ms),
    }
}

fn load_record(
    connection: &Connection,
    server_id: McpServerId,
) -> Result<Option<McpPersistedRegistryRecord>, McpRegistryPersistenceError> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS}
         FROM mcp_registry_servers
         WHERE server_id = ?1 AND record_state = 'active'"
    );
    let raw = connection
        .query_row(&sql, [server_id.to_string()], raw_record_from_row)
        .optional()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    raw.map(decode_record).transpose()
}

fn list_records(
    connection: &Connection,
) -> Result<Vec<McpPersistedRegistryRecord>, McpRegistryPersistenceError> {
    scan_raw_records(connection, true)?
        .into_iter()
        .map(|scanned| {
            scanned
                .record
                .ok_or(McpRegistryPersistenceError::CorruptRecord)
                .and_then(decode_record)
        })
        .collect()
}

fn scan_raw_records(
    connection: &Connection,
    active_only: bool,
) -> Result<Vec<ScannedRawRegistryRow>, McpRegistryPersistenceError> {
    let filter = if active_only {
        "WHERE record_state = 'active'"
    } else {
        ""
    };
    let sql = format!(
        "SELECT rowid, {SELECT_COLUMNS}
         FROM mcp_registry_servers
         {filter}
         ORDER BY rowid"
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let mut rows = statement
        .query([])
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let mut records = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
    {
        let row_id = row
            .get::<_, i64>(0)
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let revision_candidate = match row
            .get_ref(18)
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
        {
            ValueRef::Integer(value) => u64::try_from(value)
                .ok()
                .filter(|revision| *revision <= MAX_WIRE_SAFE_INTEGER),
            _ => None,
        };
        let already_invalid = matches!(
            row.get_ref(26)
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?,
            ValueRef::Text(value) if value == RECORD_STATE_INVALID.as_bytes()
        );
        records.push(ScannedRawRegistryRow {
            row_id,
            revision_candidate,
            already_invalid,
            record: raw_record_from_row_at(row, 1).ok(),
        });
    }
    Ok(records)
}

struct ScannedRawRegistryRow {
    row_id: i64,
    revision_candidate: Option<u64>,
    already_invalid: bool,
    record: Option<RawRegistryRecord>,
}

#[derive(Clone)]
struct RawRegistryRecord {
    schema_version: i64,
    server_id: String,
    display_name: String,
    scope_kind: String,
    source_kind: String,
    transport_kind: String,
    executable: String,
    arguments_json: String,
    cwd: String,
    enabled: i64,
    trust: String,
    approval_mode: String,
    connect_timeout_ms: i64,
    request_timeout_ms: i64,
    shutdown_timeout_ms: i64,
    config_digest: String,
    config_epoch: String,
    registry_revision: i64,
    launch_spec_digest: String,
    authorized_launch_spec_digest: Option<String>,
    authorized_config_epoch: Option<String>,
    authorized_config_digest: Option<String>,
    authorization_format_version: Option<i64>,
    authorization_policy_version: Option<i64>,
    authorized_at: Option<i64>,
    record_state: String,
    safe_error_code: Option<String>,
    created_at: i64,
    updated_at: i64,
    model_namespace: Option<String>,
}

fn raw_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRegistryRecord> {
    raw_record_from_row_at(row, 0)
}

fn raw_record_from_row_at(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<RawRegistryRecord> {
    Ok(RawRegistryRecord {
        schema_version: row.get(offset)?,
        server_id: row.get(offset + 1)?,
        display_name: row.get(offset + 2)?,
        scope_kind: row.get(offset + 3)?,
        source_kind: row.get(offset + 4)?,
        transport_kind: row.get(offset + 5)?,
        executable: row.get(offset + 6)?,
        arguments_json: row.get(offset + 7)?,
        cwd: row.get(offset + 8)?,
        enabled: row.get(offset + 9)?,
        trust: row.get(offset + 10)?,
        approval_mode: row.get(offset + 11)?,
        connect_timeout_ms: row.get(offset + 12)?,
        request_timeout_ms: row.get(offset + 13)?,
        shutdown_timeout_ms: row.get(offset + 14)?,
        config_digest: row.get(offset + 15)?,
        config_epoch: row.get(offset + 16)?,
        registry_revision: row.get(offset + 17)?,
        launch_spec_digest: row.get(offset + 18)?,
        authorized_launch_spec_digest: row.get(offset + 19)?,
        authorized_config_epoch: row.get(offset + 20)?,
        authorized_config_digest: row.get(offset + 21)?,
        authorization_format_version: row.get(offset + 22)?,
        authorization_policy_version: row.get(offset + 23)?,
        authorized_at: row.get(offset + 24)?,
        record_state: row.get(offset + 25)?,
        safe_error_code: row.get(offset + 26)?,
        created_at: row.get(offset + 27)?,
        updated_at: row.get(offset + 28)?,
        model_namespace: row.get(offset + 29)?,
    })
}

fn decode_record(
    raw: RawRegistryRecord,
) -> Result<McpPersistedRegistryRecord, McpRegistryPersistenceError> {
    if raw.schema_version != REGISTRY_SCHEMA_VERSION
        || raw.record_state != RECORD_STATE_ACTIVE
        || raw.source_kind != SOURCE_USER_MANUAL
        || !matches!(
            raw.safe_error_code.as_deref(),
            None | Some(SAFE_ERROR_RECONCILED)
        )
        || raw.created_at < 0
        || raw.updated_at < raw.created_at
    {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    let config = normalize_config(config_from_raw(&raw)?)?;
    let persisted_digest = McpConfigDigest::from_str(&raw.config_digest)
        .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?;
    let computed_digest =
        config_digest(&config).map_err(|_| McpRegistryPersistenceError::CorruptRecord)?;
    if persisted_digest != computed_digest {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    let config_epoch = McpConfigEpoch::from_str(&raw.config_epoch)
        .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?;
    let model_namespace = raw
        .model_namespace
        .as_deref()
        .ok_or(McpRegistryPersistenceError::CorruptRecord)
        .and_then(|value| {
            McpModelNamespace::from_str(value)
                .map_err(|_| McpRegistryPersistenceError::CorruptRecord)
        })?;
    let revision = u64::try_from(raw.registry_revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or(McpRegistryPersistenceError::CorruptRecord)?;
    let persisted_launch_digest = McpLaunchSpecDigest::from_str(&raw.launch_spec_digest)?;
    if persisted_launch_digest != compute_launch_spec_digest(&config)? {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    let authorization = decode_authorization(&raw, config.id)?;
    let record = McpPersistedRegistryRecord {
        entry: McpRegistryEntry {
            config,
            model_namespace,
            config_digest: persisted_digest,
            config_epoch,
            revision,
        },
        source: McpServerSource::UserManual,
        launch_spec_digest: persisted_launch_digest,
        launch_authorization: authorization,
        safe_error_code: raw.safe_error_code,
        created_at_ms: raw.created_at,
        updated_at_ms: raw.updated_at,
    };
    if record.entry.config.enabled && !launch_authorization_identity_is_valid(&record) {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    if record.entry.config.trust == McpTrustLevel::UserApproved
        && !launch_authorization_identity_is_valid(&record)
    {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    if record.entry.config.trust == McpTrustLevel::Untrusted
        && record.launch_authorization.is_some()
    {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    Ok(record)
}

fn config_from_raw(
    raw: &RawRegistryRecord,
) -> Result<McpServerConfig, McpRegistryPersistenceError> {
    if raw.scope_kind != "user"
        || raw.source_kind != SOURCE_USER_MANUAL
        || raw.transport_kind != "stdio"
    {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    let id = McpServerId::from_str(&raw.server_id)
        .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?;
    let arguments = serde_json::from_str::<Vec<String>>(&raw.arguments_json)
        .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?;
    Ok(McpServerConfig {
        id,
        display_name: raw.display_name.clone(),
        scope: McpServerScope::User,
        trust: parse_trust(&raw.trust)?,
        approval_mode: parse_approval_mode(&raw.approval_mode)?,
        enabled: parse_bool(raw.enabled)?,
        transport: McpTransportConfig::Stdio(McpStdioConfig {
            program: PathBuf::from(&raw.executable),
            arguments,
            cwd: PathBuf::from(&raw.cwd),
            environment: Vec::new(),
        }),
        connect_timeout_ms: parse_positive_u64(raw.connect_timeout_ms)?,
        request_timeout_ms: parse_positive_u64(raw.request_timeout_ms)?,
        shutdown_timeout_ms: parse_positive_u64(raw.shutdown_timeout_ms)?,
    })
}

fn decode_authorization(
    raw: &RawRegistryRecord,
    server_id: McpServerId,
) -> Result<Option<McpLaunchAuthorizationRecord>, McpRegistryPersistenceError> {
    match (
        &raw.authorized_launch_spec_digest,
        &raw.authorized_config_epoch,
        &raw.authorized_config_digest,
        raw.authorization_format_version,
        raw.authorization_policy_version,
        raw.authorized_at,
    ) {
        (None, None, None, None, None, None) => Ok(None),
        (
            Some(launch_digest),
            Some(config_epoch),
            Some(config_digest),
            Some(format_version),
            Some(policy_version),
            Some(authorized_at),
        ) if format_version == i64::from(MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION)
            && policy_version > 0
            && authorized_at >= 0 =>
        {
            let launch_spec_digest = McpLaunchSpecDigest::from_str(&raw.launch_spec_digest)?;
            let file_identity_digest = McpLaunchSpecDigest::from_str(launch_digest)?;
            Ok(Some(McpLaunchAuthorizationRecord {
                authorization_format_version: MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION,
                server_id,
                launch_spec_digest,
                file_identity_digest,
                authored_config_epoch: McpConfigEpoch::from_str(config_epoch)
                    .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?,
                authored_config_digest: McpConfigDigest::from_str(config_digest)
                    .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?,
                authorization_policy_version: u32::try_from(policy_version)
                    .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?,
                authorized_at_ms: authorized_at,
            }))
        }
        _ => Err(McpRegistryPersistenceError::CorruptRecord),
    }
}

fn registry_change(kind: McpRegistryChangeKind, entry: &McpRegistryEntry) -> McpRegistryChange {
    McpRegistryChange {
        revision: entry.revision,
        kind,
        server_id: entry.config.id,
        scope: entry.config.scope.clone(),
        enabled: entry.config.enabled,
        config_digest: entry.config_digest.clone(),
        config_epoch: entry.config_epoch,
    }
}

fn next_revision(transaction: &Transaction<'_>) -> Result<u64, McpRegistryPersistenceError> {
    let current = read_revision(transaction)?;
    let next = current
        .checked_add(1)
        .filter(|value| *value <= MAX_WIRE_SAFE_INTEGER)
        .ok_or(McpRegistryPersistenceError::RevisionExhausted)?;
    write_revision(transaction, next)?;
    Ok(next)
}

fn read_revision(connection: &Connection) -> Result<u64, McpRegistryPersistenceError> {
    let value = connection
        .query_row(
            "SELECT revision_watermark
             FROM mcp_registry_metadata
             WHERE singleton = ?1 AND schema_version = ?2",
            params![REGISTRY_METADATA_SINGLETON, REGISTRY_SCHEMA_VERSION],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    u64::try_from(value)
        .ok()
        .filter(|revision| *revision <= MAX_WIRE_SAFE_INTEGER)
        .ok_or(McpRegistryPersistenceError::CorruptRecord)
}

fn write_revision(
    transaction: &Transaction<'_>,
    revision: u64,
) -> Result<(), McpRegistryPersistenceError> {
    let affected = transaction
        .execute(
            "UPDATE mcp_registry_metadata
             SET revision_watermark = ?2, updated_at = ?3
             WHERE singleton = ?1 AND schema_version = ?4",
            params![
                REGISTRY_METADATA_SINGLETON,
                sqlite_revision(revision)?,
                now_ms()?,
                REGISTRY_SCHEMA_VERSION
            ],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if affected != 1 {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    Ok(())
}

fn quarantine_record_by_rowid(
    transaction: &Transaction<'_>,
    row_id: i64,
    revision: u64,
) -> Result<(), McpRegistryPersistenceError> {
    let revision = sqlite_revision(revision)?;
    let now = now_ms()?;
    let preserved = transaction.execute(
        "UPDATE mcp_registry_servers SET
                 enabled = 0,
                 trust = 'untrusted',
                 authorized_launch_spec_digest = NULL,
                 authorized_config_epoch = NULL,
                 authorized_config_digest = NULL,
                 authorization_format_version = NULL,
                 authorization_policy_version = NULL,
                 authorized_at = NULL,
                 registry_revision = ?2,
                 record_state = 'invalid',
                 safe_error_code = ?3,
                 updated_at = CASE
                     WHEN typeof(created_at) = 'integer' AND created_at >= 0
                     THEN MAX(created_at, ?4)
                     ELSE ?4
                 END
             WHERE rowid = ?1",
        params![row_id, revision, SAFE_ERROR_INVALID, now],
    );
    match preserved {
        Ok(1) => return Ok(()),
        Ok(_) => return Err(McpRegistryPersistenceError::CorruptRecord),
        Err(_) => {}
    }

    // A row inserted while SQLite constraint checking was disabled can be too
    // malformed for the preserving update above. Replace only that inert row
    // with a bounded, non-routable projection so it cannot block every other
    // valid Server during startup.
    let zero_digest = "0".repeat(64);
    let affected = transaction
        .execute(
            "UPDATE mcp_registry_servers SET
                 schema_version = 1,
                 server_id = CASE
                     WHEN typeof(server_id) = 'text' AND length(server_id) > 0
                     THEN server_id
                     ELSE '__invalid_mcp_row_' || printf('%lld_', rowid)
                          || lower(hex(randomblob(16)))
                 END,
                 display_name = 'Invalid persisted MCP server',
                 scope_kind = 'user',
                 source_kind = 'user_manual',
                 transport_kind = 'stdio',
                 executable = '/invalid',
                 arguments_json = '[]',
                 cwd = '/',
                 enabled = 0,
                 trust = 'untrusted',
                 approval_mode = 'deny',
                 connect_timeout_ms = 1000,
                 request_timeout_ms = 60000,
                 shutdown_timeout_ms = 2000,
                 config_digest = ?3,
                 config_epoch = '00000000-0000-4000-8000-000000000000',
                 registry_revision = ?2,
                 launch_spec_digest = ?3,
                 authorized_launch_spec_digest = NULL,
                 authorized_config_epoch = NULL,
                 authorized_config_digest = NULL,
                 authorization_format_version = NULL,
                 authorization_policy_version = NULL,
                 authorized_at = NULL,
                 record_state = 'invalid',
                 safe_error_code = ?4,
                 created_at = 0,
                 updated_at = ?5
             WHERE rowid = ?1",
            params![row_id, revision, zero_digest, SAFE_ERROR_INVALID, now],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if affected != 1 {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    Ok(())
}

fn bool_integer(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn parse_bool(value: i64) -> Result<bool, McpRegistryPersistenceError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(McpRegistryPersistenceError::CorruptRecord),
    }
}

fn trust_text(trust: McpTrustLevel) -> Result<&'static str, McpRegistryPersistenceError> {
    match trust {
        McpTrustLevel::Untrusted => Ok("untrusted"),
        McpTrustLevel::UserApproved => Ok("user_approved"),
        _ => Err(McpRegistryPersistenceError::InvalidConfig),
    }
}

fn parse_trust(value: &str) -> Result<McpTrustLevel, McpRegistryPersistenceError> {
    match value {
        "untrusted" => Ok(McpTrustLevel::Untrusted),
        "user_approved" => Ok(McpTrustLevel::UserApproved),
        _ => Err(McpRegistryPersistenceError::CorruptRecord),
    }
}

fn approval_mode_text(mode: McpApprovalMode) -> Result<&'static str, McpRegistryPersistenceError> {
    match mode {
        McpApprovalMode::Prompt => Ok("prompt"),
        McpApprovalMode::Auto => Ok("auto"),
        McpApprovalMode::Deny => Ok("deny"),
        _ => Err(McpRegistryPersistenceError::InvalidConfig),
    }
}

fn parse_approval_mode(value: &str) -> Result<McpApprovalMode, McpRegistryPersistenceError> {
    match value {
        "prompt" => Ok(McpApprovalMode::Prompt),
        "auto" => Ok(McpApprovalMode::Auto),
        "deny" => Ok(McpApprovalMode::Deny),
        _ => Err(McpRegistryPersistenceError::CorruptRecord),
    }
}

fn parse_positive_u64(value: i64) -> Result<u64, McpRegistryPersistenceError> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or(McpRegistryPersistenceError::CorruptRecord)
}

fn sqlite_u64(value: u64) -> Result<i64, McpRegistryPersistenceError> {
    i64::try_from(value).map_err(|_| McpRegistryPersistenceError::InvalidConfig)
}

fn sqlite_revision(value: u64) -> Result<i64, McpRegistryPersistenceError> {
    if value > MAX_WIRE_SAFE_INTEGER {
        return Err(McpRegistryPersistenceError::RevisionExhausted);
    }
    i64::try_from(value).map_err(|_| McpRegistryPersistenceError::RevisionExhausted)
}

fn now_ms() -> Result<i64, McpRegistryPersistenceError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
        .as_millis();
    i64::try_from(millis).map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use mycopilot_mcp_client::McpEnvBinding;
    use tempfile::tempdir;

    use super::*;

    const FORBIDDEN_CANARY: &str = "MCP_REGISTRY_SECRET_CANARY";

    fn config(id: McpServerId, display_name: &str) -> McpServerConfig {
        McpServerConfig {
            id,
            display_name: display_name.to_string(),
            scope: McpServerScope::User,
            trust: McpTrustLevel::Untrusted,
            approval_mode: McpApprovalMode::Prompt,
            enabled: false,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: std::env::current_exe().expect("resolve repository-owned test executable"),
                arguments: vec!["--mode".to_string(), "stdio".to_string(), String::new()],
                cwd: std::env::current_dir().expect("resolve repository-owned test cwd"),
                environment: Vec::new(),
            }),
            connect_timeout_ms: 1_000,
            request_timeout_ms: 60_000,
            shutdown_timeout_ms: 2_000,
        }
    }

    #[test]
    fn migration_preserves_existing_database_and_uses_explicit_columns() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "PRAGMA user_version = 73;
                 CREATE TABLE existing_application_data (
                     id INTEGER PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT INTO existing_application_data(value) VALUES ('preserve-me');",
            )
            .unwrap();
        drop(legacy);

        let registry = SqliteMcpRegistry::open(&path).unwrap();
        assert_eq!(registry.current_revision().unwrap(), 0);
        drop(registry);

        let inspection = Connection::open(&path).unwrap();
        assert_eq!(
            inspection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            73
        );
        assert_eq!(
            inspection
                .query_row("SELECT value FROM existing_application_data", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "preserve-me"
        );
        let columns = inspection
            .prepare("PRAGMA table_info(mcp_registry_servers)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(!columns.iter().any(|column| {
            matches!(
                column.as_str(),
                "config_json"
                    | "environment"
                    | "headers"
                    | "bearer_token"
                    | "oauth"
                    | "stderr"
                    | "tool_result"
                    | "tool_arguments"
            )
        }));
    }

    #[test]
    fn migration_adds_auto_mode_without_changing_v1_identity_or_authorization() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let prompt_id = McpServerId::new();
        let deny_id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();

        let prompt = registry
            .add_persisted(config(prompt_id, "prompt fixture"))
            .unwrap();
        let prompt_record = registry.get_persisted(prompt_id).unwrap().unwrap();
        let file_identity = compute_launch_file_identity_digest(
            &prompt_record.entry.config,
            &prompt_record.launch_spec_digest,
        )
        .unwrap();
        registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&prompt),
                &prompt_record.launch_spec_digest,
                &file_identity,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                1_785_384_000_000,
            )
            .unwrap();
        let authorized = registry.get_persisted(prompt_id).unwrap().unwrap();
        let enabled = registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&authorized.entry),
                true,
            )
            .unwrap();
        let before = registry.get_persisted(prompt_id).unwrap().unwrap();

        let mut deny_config = config(deny_id, "deny fixture");
        deny_config.approval_mode = McpApprovalMode::Deny;
        let deny = registry.add_persisted(deny_config).unwrap();
        let revision_before_restart = registry.current_revision().unwrap();
        drop(registry);

        let connection = Connection::open(&path).unwrap();
        let current_sql = connection
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'table' AND name = 'mcp_registry_servers'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        let legacy_sql = current_sql.replace(
            "approval_mode IN ('prompt', 'auto', 'deny')",
            "approval_mode IN ('prompt', 'deny')",
        );
        assert_ne!(legacy_sql, current_sql);
        let columns = REGISTRY_SERVER_COLUMNS.join(", ");
        connection
            .execute_batch(
                "ALTER TABLE mcp_registry_servers
                     RENAME TO mcp_registry_servers_auto_source;
                 DROP INDEX mcp_registry_servers_revision;",
            )
            .unwrap();
        connection.execute_batch(&legacy_sql).unwrap();
        connection
            .execute_batch(&format!(
                "INSERT INTO mcp_registry_servers ({columns})
                 SELECT {columns} FROM mcp_registry_servers_auto_source;
                 DROP TABLE mcp_registry_servers_auto_source;
                 CREATE INDEX mcp_registry_servers_revision
                 ON mcp_registry_servers(registry_revision, server_id);"
            ))
            .unwrap();
        drop(connection);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        assert_eq!(
            reopened.current_revision().unwrap(),
            revision_before_restart
        );
        let restored_prompt = reopened.get_persisted(prompt_id).unwrap().unwrap();
        assert_eq!(restored_prompt.entry.config.id, prompt_id);
        assert_eq!(
            restored_prompt.entry.config_epoch,
            before.entry.config_epoch
        );
        assert_eq!(
            restored_prompt.entry.config_digest,
            before.entry.config_digest
        );
        assert_eq!(restored_prompt.entry.revision, enabled.revision);
        assert!(restored_prompt.entry.config.enabled);
        assert_eq!(
            restored_prompt.entry.config.approval_mode,
            McpApprovalMode::Prompt
        );
        assert_eq!(
            restored_prompt.launch_authorization,
            before.launch_authorization
        );
        let restored_deny = reopened.get_persisted(deny_id).unwrap().unwrap();
        assert_eq!(restored_deny.entry.config_epoch, deny.config_epoch);
        assert_eq!(restored_deny.entry.config_digest, deny.config_digest);
        assert_eq!(restored_deny.entry.revision, deny.revision);
        assert_eq!(
            restored_deny.entry.config.approval_mode,
            McpApprovalMode::Deny
        );

        let mut auto_config = restored_prompt.entry.config.clone();
        auto_config.approval_mode = McpApprovalMode::Auto;
        let updated = match reopened
            .update_with_precondition(
                &McpRegistryMutationPrecondition::from_entry(&restored_prompt.entry),
                auto_config,
            )
            .unwrap()
        {
            McpRegistryMutation::Updated(entry) => entry,
            other => panic!("approval-mode change must update the record: {other:?}"),
        };
        assert_eq!(updated.config.id, prompt_id);
        assert_eq!(updated.config.approval_mode, McpApprovalMode::Auto);
        assert_ne!(updated.config_epoch, before.entry.config_epoch);
        assert_ne!(updated.config_digest, before.entry.config_digest);
        assert!(updated.revision > revision_before_restart);
        let persisted_auto = reopened.get_persisted(prompt_id).unwrap().unwrap();
        assert_eq!(
            persisted_auto.entry.config.approval_mode,
            McpApprovalMode::Auto
        );
        assert!(persisted_auto.launch_authorization.is_some());
    }

    #[test]
    fn migration_quarantines_an_incompatible_same_name_table_without_losing_its_data() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        drop(registry);

        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "DROP TABLE mcp_registry_servers;
                 CREATE TABLE mcp_registry_servers (
                     server_id TEXT PRIMARY KEY,
                     legacy_value TEXT NOT NULL
                 );
                 INSERT INTO mcp_registry_servers(server_id, legacy_value)
                 VALUES ('legacy-server', 'LEGACY_MCP_ROW_CANARY');",
            )
            .unwrap();
        drop(legacy);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        let (revision, records) = reopened.snapshot().unwrap();
        assert_eq!(revision, 0);
        assert!(records.is_empty());
        drop(reopened);

        let inspection = Connection::open(&path).unwrap();
        assert_eq!(
            inspection
                .query_row(
                    "SELECT legacy_value
                     FROM mcp_registry_servers_incompatible_v1_1
                     WHERE server_id = 'legacy-server'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "LEGACY_MCP_ROW_CANARY"
        );
        let current_columns = inspection
            .prepare("PRAGMA table_info(mcp_registry_servers)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(column_names_match(
            &current_columns,
            REGISTRY_SERVER_COLUMNS
        ));
    }

    #[test]
    fn registry_restores_identity_and_advances_revision_across_restart() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let first = registry.add(config(id, "first")).unwrap();
        let first_epoch = first.config_epoch;
        let first_digest = first.config_digest.clone();
        let first_namespace = first.model_namespace.clone();
        assert_eq!(first.revision, 1);
        drop(registry);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        let restored = reopened.get(id).unwrap().unwrap();
        assert_eq!(restored.config_epoch, first_epoch);
        assert_eq!(restored.config_digest, first_digest);
        assert_eq!(restored.model_namespace, first_namespace);
        assert_eq!(restored.revision, 1);
        assert_eq!(reopened.current_revision().unwrap(), 1);

        let mut updated = restored.config.clone();
        updated.display_name = "updated".to_string();
        let changed = reopened
            .update_with_precondition(
                &McpRegistryMutationPrecondition::from_entry(&restored),
                updated,
            )
            .unwrap();
        let McpRegistryMutation::Updated(changed) = changed else {
            panic!("expected updated record");
        };
        assert_eq!(changed.revision, 2);
        assert_ne!(changed.config_epoch, first_epoch);
        assert_ne!(changed.config_digest, first_digest);
        assert_eq!(changed.model_namespace, first_namespace);
        drop(reopened);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        assert_eq!(reopened.current_revision().unwrap(), 2);
        assert_eq!(reopened.get(id).unwrap().unwrap(), changed);
    }

    #[test]
    fn legacy_database_backfills_model_namespace_without_changing_authorization_identity() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let added = registry.add(config(id, "Filesystem Test")).unwrap();
        let persisted = registry.get_persisted(id).unwrap().unwrap();
        let file_identity_digest = compute_launch_file_identity_digest(
            &persisted.entry.config,
            &persisted.launch_spec_digest,
        )
        .unwrap();
        registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&added),
                &persisted.launch_spec_digest,
                &file_identity_digest,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                1,
            )
            .unwrap();
        let authorized = registry.get_persisted(id).unwrap().unwrap();
        let enabled = registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&authorized.entry),
                true,
            )
            .unwrap();
        let before = registry.get_persisted(id).unwrap().unwrap();
        assert_eq!(enabled.model_namespace.as_str(), "filesystem_test");
        drop(registry);

        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch("DROP TABLE mcp_registry_model_namespaces;")
            .unwrap();
        drop(legacy);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        let after = reopened.get_persisted(id).unwrap().unwrap();
        assert_eq!(after.entry.model_namespace.as_str(), "filesystem_test");
        assert_eq!(after.entry.config_digest, before.entry.config_digest);
        assert_eq!(after.entry.config_epoch, before.entry.config_epoch);
        assert_eq!(after.entry.revision, before.entry.revision);
        assert_eq!(after.launch_authorization, before.launch_authorization);
        assert!(after.entry.config.enabled);
        assert_eq!(reopened.current_revision().unwrap(), before.entry.revision);
    }

    #[test]
    fn malformed_namespace_storage_class_is_repaired_without_hiding_healthy_servers() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let damaged_id = McpServerId::new();
        let healthy_id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        registry.add(config(damaged_id, "Filesystem Test")).unwrap();
        let healthy = registry.add(config(healthy_id, "Memory Test")).unwrap();
        drop(registry);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE mcp_registry_model_namespaces
                 SET model_namespace = x'00ff'
                 WHERE server_id = ?1",
                [damaged_id.to_string()],
            )
            .unwrap();
        drop(connection);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        let records = reopened.list().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            reopened.get(damaged_id).unwrap().unwrap().model_namespace,
            McpModelNamespace::from_str("filesystem_test").unwrap()
        );
        assert_eq!(
            reopened.get(healthy_id).unwrap().unwrap().model_namespace,
            healthy.model_namespace
        );
    }

    #[test]
    fn noncanonical_namespace_table_is_rebuilt_with_primary_and_unique_constraints() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let first_id = McpServerId::new();
        let second_id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        registry.add(config(first_id, "Filesystem Test")).unwrap();
        registry.add(config(second_id, "Filesystem/Test")).unwrap();
        drop(registry);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE mcp_registry_model_namespaces;
                 CREATE TABLE mcp_registry_model_namespaces (
                     schema_version INTEGER,
                     server_id TEXT,
                     model_namespace TEXT,
                     created_at INTEGER
                 );",
            )
            .unwrap();
        for server_id in [first_id, second_id] {
            connection
                .execute(
                    "INSERT INTO mcp_registry_model_namespaces (
                         schema_version, server_id, model_namespace, created_at
                     ) VALUES (1, ?1, 'filesystem_test', 1)",
                    [server_id.to_string()],
                )
                .unwrap();
        }
        drop(connection);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        let first = reopened.get(first_id).unwrap().unwrap();
        let second = reopened.get(second_id).unwrap().unwrap();
        assert_ne!(first.model_namespace, second.model_namespace);
        assert!([
            first.model_namespace.as_str(),
            second.model_namespace.as_str()
        ]
        .contains(&"filesystem_test"));
        drop(reopened);

        let connection = Connection::open(&path).unwrap();
        assert!(connection
            .execute(
                "INSERT INTO mcp_registry_model_namespaces (
                     schema_version, server_id, model_namespace, created_at
                 ) VALUES (1, 'duplicate-server', 'filesystem_test', 1)",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO mcp_registry_model_namespaces (
                     schema_version, server_id, model_namespace, created_at
                 ) VALUES (1, ?1, 'another_namespace', 1)",
                [first_id.to_string()],
            )
            .is_err());
    }

    #[test]
    fn structurally_misleading_namespace_tables_are_quarantined_and_rebuilt() {
        let cases = [
            (
                "without-rowid",
                "CREATE TABLE mcp_registry_model_namespaces (
                     schema_version INTEGER NOT NULL,
                     server_id TEXT PRIMARY KEY,
                     model_namespace TEXT NOT NULL UNIQUE,
                     created_at INTEGER NOT NULL
                 ) WITHOUT\nROWID;",
            ),
            (
                "compound-primary-key",
                "CREATE TABLE mcp_registry_model_namespaces (
                     schema_version INTEGER NOT NULL,
                     server_id TEXT NOT NULL,
                     model_namespace TEXT NOT NULL UNIQUE,
                     created_at INTEGER NOT NULL,
                     PRIMARY KEY (server_id, created_at)
                 );",
            ),
            (
                "partial-unique-index",
                "CREATE TABLE mcp_registry_model_namespaces (
                     schema_version INTEGER NOT NULL,
                     server_id TEXT PRIMARY KEY,
                     model_namespace TEXT NOT NULL,
                     created_at INTEGER NOT NULL
                 );
                 CREATE UNIQUE INDEX misleading_namespace_unique
                 ON mcp_registry_model_namespaces(model_namespace)
                 WHERE created_at < 0;",
            ),
            (
                "expression-unique-index",
                "CREATE TABLE mcp_registry_model_namespaces (
                     schema_version INTEGER NOT NULL,
                     server_id TEXT PRIMARY KEY,
                     model_namespace TEXT NOT NULL,
                     created_at INTEGER NOT NULL
                 );
                 CREATE UNIQUE INDEX misleading_namespace_expression_unique
                 ON mcp_registry_model_namespaces(lower(model_namespace));",
            ),
        ];

        for (case, replacement_schema) in cases {
            let directory = tempdir().unwrap();
            let path = directory.path().join(format!("{case}.sqlite"));
            let id = McpServerId::new();
            let registry = SqliteMcpRegistry::open(&path).unwrap();
            registry.add(config(id, "Filesystem Test")).unwrap();
            drop(registry);

            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(&format!(
                    "DROP TABLE mcp_registry_model_namespaces; {replacement_schema}"
                ))
                .unwrap();
            connection
                .execute(
                    "INSERT INTO mcp_registry_model_namespaces (
                         schema_version, server_id, model_namespace, created_at
                     ) VALUES (1, ?1, 'filesystem_test', 1)",
                    [id.to_string()],
                )
                .unwrap();
            drop(connection);

            let reopened = SqliteMcpRegistry::open(&path).unwrap_or_else(|error| {
                panic!("{case} namespace table should be rebuilt: {error:?}")
            });
            let restored = reopened.get(id).unwrap().unwrap();
            assert_eq!(restored.model_namespace.as_str(), "filesystem_test");
            drop(reopened);

            let connection = Connection::open(&path).unwrap();
            assert!(connection
                .execute(
                    "INSERT INTO mcp_registry_model_namespaces (
                         schema_version, server_id, model_namespace, created_at
                     ) VALUES (1, 'another-server', 'filesystem_test', 1)",
                    [],
                )
                .is_err());
        }
    }

    #[test]
    fn snapshot_pairs_records_with_the_same_revision_watermark_under_writes() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let registry = Arc::new(SqliteMcpRegistry::open(&path).unwrap());
        let writer_registry = Arc::clone(&registry);
        let writer = std::thread::spawn(move || {
            for index in 0..64 {
                writer_registry
                    .add_persisted(config(McpServerId::new(), &format!("snapshot-{index}")))
                    .unwrap();
            }
        });

        for _ in 0..10_000 {
            let (revision, records) = registry.snapshot().unwrap();
            assert_eq!(revision, records.len() as u64);
            if revision == 64 {
                break;
            }
            std::thread::yield_now();
        }
        writer.join().unwrap();
        let (revision, records) = registry.snapshot().unwrap();
        assert_eq!(revision, 64);
        assert_eq!(records.len(), 64);
    }

    #[test]
    fn registry_revision_never_exceeds_the_javascript_safe_integer_boundary() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        {
            let connection = registry.lock_connection().unwrap();
            connection
                .execute(
                    "UPDATE mcp_registry_metadata
                     SET revision_watermark = ?1
                     WHERE singleton = ?2",
                    params![
                        i64::try_from(MAX_WIRE_SAFE_INTEGER).unwrap(),
                        REGISTRY_METADATA_SINGLETON
                    ],
                )
                .unwrap();
        }
        let mut changes = registry.subscribe();
        assert_eq!(
            registry
                .add_persisted(config(McpServerId::new(), "revision-overflow"))
                .unwrap_err(),
            McpRegistryPersistenceError::RevisionExhausted
        );
        assert_eq!(registry.current_revision().unwrap(), MAX_WIRE_SAFE_INTEGER);
        assert!(registry.list_persisted().unwrap().is_empty());
        let no_event = tokio_test_block_on(async {
            tokio::time::timeout(Duration::from_millis(20), changes.recv()).await
        });
        assert!(no_event.is_err());
    }

    #[test]
    fn remove_and_readd_never_reuse_identity_or_revision() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let first = registry.add(config(id, "same")).unwrap();
        let removed = registry.remove(id).unwrap().unwrap();
        assert_eq!(removed.revision, 2);
        let second = registry.add(config(id, "same")).unwrap();
        assert_eq!(second.revision, 3);
        assert_ne!(second.config_epoch, first.config_epoch);
        assert_eq!(second.config_digest, first.config_digest);
    }

    #[test]
    fn environment_is_rejected_and_never_persisted() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let mut invalid = config(McpServerId::new(), "invalid");
        let McpTransportConfig::Stdio(stdio) = &mut invalid.transport else {
            panic!("test config must use stdio");
        };
        stdio.environment.push(McpEnvBinding::Plain {
            name: "NEUTRAL".to_string(),
            value: FORBIDDEN_CANARY.to_string(),
        });
        assert!(registry.add(invalid).is_err());
        drop(registry);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(FORBIDDEN_CANARY));
    }

    #[test]
    fn exact_launch_authorization_preserves_empty_and_ordered_arguments() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let added = registry.add(config(id, "authorized")).unwrap();
        assert_eq!(
            registry
                .set_enabled(&McpRegistryMutationPrecondition::from_entry(&added), true)
                .unwrap_err(),
            McpRegistryPersistenceError::AuthorizationRequired
        );
        let before = registry.get_persisted(id).unwrap().unwrap();
        let file_identity_digest =
            compute_launch_file_identity_digest(&before.entry.config, &before.launch_spec_digest)
                .unwrap();
        let authorization = registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&before.entry),
                &before.launch_spec_digest,
                &file_identity_digest,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                123_456,
            )
            .unwrap();
        assert_eq!(authorization.server_id, id);
        assert_eq!(
            authorization.authorization_format_version,
            MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION
        );
        assert_eq!(
            authorization.authorization_policy_version,
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION
        );
        let inspection = Connection::open(&path).unwrap();
        assert_eq!(
            inspection
                .query_row(
                    "SELECT authorization_format_version
                     FROM mcp_registry_servers WHERE server_id = ?1",
                    [id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            i64::from(MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION)
        );
        drop(inspection);
        let authorized_entry = registry.get(id).unwrap().unwrap();
        assert_eq!(authorized_entry.config.trust, McpTrustLevel::UserApproved);
        let enabled = registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&authorized_entry),
                true,
            )
            .unwrap();
        assert!(enabled.config.enabled);
        assert_eq!(
            registry
                .get_persisted(id)
                .unwrap()
                .unwrap()
                .launch_authorization
                .unwrap()
                .launch_spec_digest,
            authorization.launch_spec_digest
        );

        let mut launch_drift = enabled.config.clone();
        let McpTransportConfig::Stdio(stdio) = &mut launch_drift.transport else {
            panic!("test config must use stdio");
        };
        stdio.arguments.swap(0, 1);
        let changed = registry
            .update_with_precondition(
                &McpRegistryMutationPrecondition::from_entry(&enabled),
                launch_drift,
            )
            .unwrap();
        let McpRegistryMutation::Updated(changed) = changed else {
            panic!("expected launch mutation");
        };
        assert!(!changed.config.enabled);
        assert_eq!(changed.config.trust, McpTrustLevel::Untrusted);
        assert!(registry
            .get_persisted(id)
            .unwrap()
            .unwrap()
            .launch_authorization
            .is_none());
    }

    #[test]
    fn launch_identity_tracks_code_not_mutable_data_and_never_blocks_disable() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("owned-fixture-executable");
        let script = directory.path().join("owned-server.js");
        let mutable_data = directory.path().join("owned-fixture-data");
        std::fs::write(&executable, b"owned executable version one").unwrap();
        std::fs::write(&script, b"owned script version one").unwrap();
        std::fs::write(&mutable_data, b"mutable data version one").unwrap();
        let id = McpServerId::new();
        let mut server = config(id, "selective identity");
        let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
            panic!("test config must use stdio");
        };
        stdio.program = executable.clone();
        stdio.arguments = vec![
            script.to_string_lossy().into_owned(),
            mutable_data.to_string_lossy().into_owned(),
            "--config=missing.js".to_string(),
            "https://owned.invalid/remote-server.js".to_string(),
        ];
        stdio.cwd = directory.path().to_path_buf();
        let registry = SqliteMcpRegistry::open(directory.path().join("registry.sqlite")).unwrap();
        let added = registry.add(server).unwrap();
        let persisted = registry.get_persisted(id).unwrap().unwrap();
        let file_identity_digest = compute_launch_file_identity_digest(
            &persisted.entry.config,
            &persisted.launch_spec_digest,
        )
        .unwrap();
        registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&added),
                &persisted.launch_spec_digest,
                &file_identity_digest,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                1,
            )
            .unwrap();

        // Extensionless runtime data and unknown option values are not
        // misclassified as executable code authority.
        std::fs::write(&mutable_data, b"mutable data version two with a new size").unwrap();
        let authorized = registry.get(id).unwrap().unwrap();
        let enabled = registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&authorized),
                true,
            )
            .unwrap();

        // Executable replacement invalidates future starts, but stale launch
        // identity must never prevent the fail-closed disable path.
        std::fs::write(&executable, b"owned executable version two with a new size").unwrap();
        let disabled = registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&enabled),
                false,
            )
            .unwrap();
        assert!(!disabled.config.enabled);
        assert_eq!(
            registry
                .set_enabled(
                    &McpRegistryMutationPrecondition::from_entry(&disabled),
                    true,
                )
                .unwrap_err(),
            McpRegistryPersistenceError::AuthorizationRequired
        );
    }

    #[cfg(unix)]
    #[test]
    fn launch_identity_ignores_metadata_only_ctime_drift() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let directory = tempdir().unwrap();
        let executable = directory.path().join("owned-fixture-executable");
        std::fs::write(&executable, b"stable owned executable bytes").unwrap();
        let mut server = config(McpServerId::new(), "stable physical identity");
        let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
            panic!("test config must use stdio");
        };
        stdio.program = executable.clone();
        stdio.cwd = directory.path().to_path_buf();

        let launch_digest = compute_launch_spec_digest(&server).unwrap();
        let first = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
        let before = std::fs::metadata(&executable).unwrap();
        let original_mode = before.permissions().mode();
        let mut changed_permissions = before.permissions();
        changed_permissions.set_mode(original_mode ^ 0o100);
        std::fs::set_permissions(&executable, changed_permissions).unwrap();
        let mut restored_permissions = std::fs::metadata(&executable).unwrap().permissions();
        restored_permissions.set_mode(original_mode);
        std::fs::set_permissions(&executable, restored_permissions).unwrap();
        let after = std::fs::metadata(&executable).unwrap();
        assert_ne!(
            (before.ctime(), before.ctime_nsec()),
            (after.ctime(), after.ctime_nsec()),
            "the fixture must exercise a ctime-only identity change"
        );

        let second = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
        assert_eq!(
            first, second,
            "metadata bookkeeping must not revoke unchanged launch bytes"
        );
    }

    #[cfg(unix)]
    #[test]
    fn authorized_server_survives_metadata_only_ctime_drift_across_registry_restart() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let database = directory.path().join("registry.sqlite");
        let executable = directory.path().join("owned-fixture-executable");
        std::fs::write(&executable, b"stable owned executable bytes").unwrap();
        let id = McpServerId::new();
        {
            let registry = SqliteMcpRegistry::open(&database).unwrap();
            let mut server = config(id, "restart-stable physical identity");
            let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
                panic!("test config must use stdio");
            };
            stdio.program = executable.clone();
            stdio.cwd = directory.path().to_path_buf();
            let added = registry.add(server).unwrap();
            let persisted = registry.get_persisted(id).unwrap().unwrap();
            let file_identity_digest = compute_launch_file_identity_digest(
                &persisted.entry.config,
                &persisted.launch_spec_digest,
            )
            .unwrap();
            registry
                .authorize_launch(
                    &McpRegistryMutationPrecondition::from_entry(&added),
                    &persisted.launch_spec_digest,
                    &file_identity_digest,
                    MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                    1,
                )
                .unwrap();
            let authorized = registry.get(id).unwrap().unwrap();
            registry
                .set_enabled(
                    &McpRegistryMutationPrecondition::from_entry(&authorized),
                    true,
                )
                .unwrap();
        }

        let before = std::fs::metadata(&executable).unwrap();
        let original_mode = before.permissions().mode();
        let mut changed_permissions = before.permissions();
        changed_permissions.set_mode(original_mode ^ 0o100);
        std::fs::set_permissions(&executable, changed_permissions).unwrap();
        let mut restored_permissions = std::fs::metadata(&executable).unwrap().permissions();
        restored_permissions.set_mode(original_mode);
        std::fs::set_permissions(&executable, restored_permissions).unwrap();

        let reopened = SqliteMcpRegistry::open(&database).unwrap();
        let recovered = reopened.get_persisted(id).unwrap().unwrap();
        assert!(recovered.entry.config.enabled);
        assert_eq!(recovered.entry.config.trust, McpTrustLevel::UserApproved);
        assert!(recovered.launch_authorization.is_some());
        assert!(launch_authorization_is_valid(&recovered));
    }

    #[test]
    fn launch_identity_hashes_same_length_content_replacement() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("owned-fixture-executable");
        std::fs::write(&executable, b"owned executable version 1").unwrap();
        let mut server = config(McpServerId::new(), "content-bound identity");
        let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
            panic!("test config must use stdio");
        };
        stdio.program = executable.clone();
        stdio.cwd = directory.path().to_path_buf();

        let launch_digest = compute_launch_spec_digest(&server).unwrap();
        let first = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
        std::fs::write(&executable, b"owned executable version 2").unwrap();
        let second = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();

        assert_ne!(first, second, "launch authority must remain content-bound");
    }

    #[test]
    fn launch_identity_bounds_content_hashing_for_large_runtime_binaries() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("oversized-owned-fixture-executable");
        let file = std::fs::File::create(&executable).unwrap();
        file.set_len(MAX_LAUNCH_CONTENT_HASH_FILE_BYTES + 1)
            .unwrap();
        let mut server = config(McpServerId::new(), "bounded physical identity");
        let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
            panic!("test config must use stdio");
        };
        stdio.program = executable;
        stdio.cwd = directory.path().to_path_buf();

        let launch_digest = compute_launch_spec_digest(&server).unwrap();
        compute_launch_file_identity_digest(&server, &launch_digest)
            .expect("large binaries use bounded metadata identity without reading sparse contents");
    }

    #[test]
    fn explicit_inline_code_paths_are_identity_bound() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("owned-fixture-executable");
        let imported = directory.path().join("owned-import.js");
        std::fs::write(&executable, b"owned executable").unwrap();
        std::fs::write(&imported, b"owned import version one").unwrap();
        let mut server = config(McpServerId::new(), "inline code identity");
        let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
            panic!("test config must use stdio");
        };
        stdio.program = executable;
        stdio.cwd = directory.path().to_path_buf();
        stdio.arguments = vec![
            format!("--import={}", imported.to_string_lossy()),
            "--import=file:///owned/remote-style.js".to_string(),
            "--loader=owned-package.js".to_string(),
        ];
        let launch_digest = compute_launch_spec_digest(&server).unwrap();
        let first = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
        std::fs::write(&imported, b"owned import version two with a new size").unwrap();
        let second = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn stale_mutation_precondition_fails_without_an_event() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let mut changes = registry.subscribe();
        let added = registry.add(config(id, "first")).unwrap();
        let first_change = tokio_test_block_on(changes.recv()).unwrap();
        assert_eq!(first_change.kind, McpRegistryChangeKind::Added);
        let stale = McpRegistryMutationPrecondition::from_entry(&added);
        let mut update = added.config.clone();
        update.display_name = "second".to_string();
        registry
            .update_with_precondition(&stale, update.clone())
            .unwrap();
        let _ = tokio_test_block_on(changes.recv()).unwrap();
        update.display_name = "third".to_string();
        assert_eq!(
            registry
                .update_with_precondition(&stale, update)
                .unwrap_err(),
            McpRegistryPersistenceError::Conflict
        );
        let no_event = tokio_test_block_on(async {
            tokio::time::timeout(Duration::from_millis(20), changes.recv()).await
        });
        assert!(no_event.is_err());
    }

    #[test]
    fn failed_database_mutation_rolls_back_revision_and_publishes_no_event() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let added = registry.add(config(id, "first")).unwrap();
        let mut changes = registry.subscribe();
        {
            let connection = registry.lock_connection().unwrap();
            connection
                .execute_batch(
                    "CREATE TRIGGER reject_mcp_registry_update
                     BEFORE UPDATE ON mcp_registry_servers
                     BEGIN
                         SELECT RAISE(ABORT, 'owned test rejection');
                     END;",
                )
                .unwrap();
        }
        let mut update = added.config.clone();
        update.display_name = "second".to_string();
        assert_eq!(
            registry
                .update_with_precondition(
                    &McpRegistryMutationPrecondition::from_entry(&added),
                    update,
                )
                .unwrap_err(),
            McpRegistryPersistenceError::StorageUnavailable
        );
        assert_eq!(registry.current_revision().unwrap(), added.revision);
        assert_eq!(registry.get(id).unwrap().unwrap(), added);
        let no_event = tokio_test_block_on(async {
            tokio::time::timeout(Duration::from_millis(20), changes.recv()).await
        });
        assert!(no_event.is_err());
    }

    #[test]
    fn non_launch_update_preserves_authorization_but_launch_change_revokes_it() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let added = registry.add(config(id, "first")).unwrap();
        let persisted = registry.get_persisted(id).unwrap().unwrap();
        let file_identity_digest = compute_launch_file_identity_digest(
            &persisted.entry.config,
            &persisted.launch_spec_digest,
        )
        .unwrap();
        let authorization = registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&added),
                &persisted.launch_spec_digest,
                &file_identity_digest,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                1,
            )
            .unwrap();
        let authorized = registry.get(id).unwrap().unwrap();

        let mut rename = authorized.config.clone();
        rename.display_name = "renamed".to_string();
        let renamed = registry
            .update_with_precondition(
                &McpRegistryMutationPrecondition::from_entry(&authorized),
                rename,
            )
            .unwrap();
        let McpRegistryMutation::Updated(renamed) = renamed else {
            panic!("expected update");
        };
        let after_rename = registry.get_persisted(id).unwrap().unwrap();
        assert_eq!(renamed.model_namespace, authorized.model_namespace);
        assert_eq!(
            after_rename
                .launch_authorization
                .as_ref()
                .unwrap()
                .launch_spec_digest,
            authorization.launch_spec_digest
        );
        assert_eq!(renamed.config.trust, McpTrustLevel::UserApproved);

        let mut launch_change = renamed.config.clone();
        let McpTransportConfig::Stdio(stdio) = &mut launch_change.transport else {
            panic!("test config must use stdio");
        };
        stdio.arguments.pop();
        let changed = registry
            .update_with_precondition(
                &McpRegistryMutationPrecondition::from_entry(&renamed),
                launch_change,
            )
            .unwrap();
        let McpRegistryMutation::Updated(changed) = changed else {
            panic!("expected update");
        };
        assert!(!changed.config.enabled);
        assert_eq!(changed.config.trust, McpTrustLevel::Untrusted);
        assert!(registry
            .get_persisted(id)
            .unwrap()
            .unwrap()
            .launch_authorization
            .is_none());
    }

    #[test]
    fn startup_invalidates_legacy_launch_authorization_without_losing_configuration() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        {
            let registry = SqliteMcpRegistry::open(&path).unwrap();
            let added = registry.add(config(id, "legacy-authorization")).unwrap();
            let persisted = registry.get_persisted(id).unwrap().unwrap();
            let file_identity_digest = compute_launch_file_identity_digest(
                &persisted.entry.config,
                &persisted.launch_spec_digest,
            )
            .unwrap();
            registry
                .authorize_launch(
                    &McpRegistryMutationPrecondition::from_entry(&added),
                    &persisted.launch_spec_digest,
                    &file_identity_digest,
                    MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                    1,
                )
                .unwrap();
            let authorized = registry.get(id).unwrap().unwrap();
            registry
                .set_enabled(
                    &McpRegistryMutationPrecondition::from_entry(&authorized),
                    true,
                )
                .unwrap();
        }
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE mcp_registry_servers
                 SET authorization_format_version = 1,
                     authorization_policy_version = 1,
                     authorized_launch_spec_digest = launch_spec_digest
                 WHERE server_id = ?1",
                [id.to_string()],
            )
            .unwrap();
        drop(connection);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        let recovered = reopened.get_persisted(id).unwrap().unwrap();
        assert_eq!(recovered.entry.config.display_name, "legacy-authorization");
        assert!(!recovered.entry.config.enabled);
        assert_eq!(recovered.entry.config.trust, McpTrustLevel::Untrusted);
        assert!(recovered.launch_authorization.is_none());
        assert_eq!(
            recovered.safe_error_code.as_deref(),
            Some(SAFE_ERROR_RECONCILED)
        );
    }

    #[test]
    fn startup_requires_one_time_reauthorization_for_previous_physical_identity_policy() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        {
            let registry = SqliteMcpRegistry::open(&path).unwrap();
            let added = registry
                .add(config(id, "previous-policy-authorization"))
                .unwrap();
            let persisted = registry.get_persisted(id).unwrap().unwrap();
            let file_identity_digest = compute_launch_file_identity_digest(
                &persisted.entry.config,
                &persisted.launch_spec_digest,
            )
            .unwrap();
            registry
                .authorize_launch(
                    &McpRegistryMutationPrecondition::from_entry(&added),
                    &persisted.launch_spec_digest,
                    &file_identity_digest,
                    MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                    1,
                )
                .unwrap();
            let authorized = registry.get(id).unwrap().unwrap();
            registry
                .set_enabled(
                    &McpRegistryMutationPrecondition::from_entry(&authorized),
                    true,
                )
                .unwrap();
        }
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE mcp_registry_servers
                 SET authorization_policy_version = 2
                 WHERE server_id = ?1",
                [id.to_string()],
            )
            .unwrap();
        drop(connection);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        let recovered = reopened.get_persisted(id).unwrap().unwrap();
        assert_eq!(
            recovered.entry.config.display_name,
            "previous-policy-authorization"
        );
        assert!(!recovered.entry.config.enabled);
        assert_eq!(recovered.entry.config.trust, McpTrustLevel::Untrusted);
        assert!(recovered.launch_authorization.is_none());
        assert_eq!(
            recovered.safe_error_code.as_deref(),
            Some(SAFE_ERROR_RECONCILED)
        );
    }

    #[test]
    fn startup_disables_v2_authorization_after_executable_replacement() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let executable = directory.path().join("owned-startup-executable");
        std::fs::write(&executable, b"owned executable version one").unwrap();
        let id = McpServerId::new();
        {
            let registry = SqliteMcpRegistry::open(&path).unwrap();
            let mut server = config(id, "startup physical drift");
            let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
                panic!("test config must use stdio");
            };
            stdio.program = executable.clone();
            stdio.cwd = directory.path().to_path_buf();
            let added = registry.add(server).unwrap();
            let persisted = registry.get_persisted(id).unwrap().unwrap();
            let file_identity_digest = compute_launch_file_identity_digest(
                &persisted.entry.config,
                &persisted.launch_spec_digest,
            )
            .unwrap();
            registry
                .authorize_launch(
                    &McpRegistryMutationPrecondition::from_entry(&added),
                    &persisted.launch_spec_digest,
                    &file_identity_digest,
                    MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                    1,
                )
                .unwrap();
            let authorized = registry.get(id).unwrap().unwrap();
            registry
                .set_enabled(
                    &McpRegistryMutationPrecondition::from_entry(&authorized),
                    true,
                )
                .unwrap();
        }

        std::fs::write(
            &executable,
            b"owned executable version two with a different size",
        )
        .unwrap();
        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        let recovered = reopened.get_persisted(id).unwrap().unwrap();
        assert!(!recovered.entry.config.enabled);
        assert_eq!(recovered.entry.config.trust, McpTrustLevel::Untrusted);
        assert!(recovered.launch_authorization.is_none());
        assert_eq!(
            recovered.safe_error_code.as_deref(),
            Some(SAFE_ERROR_RECONCILED)
        );
    }

    #[test]
    fn exact_launch_digest_binds_server_and_argument_boundaries() {
        let first_id = McpServerId::new();
        let first = config(first_id, "first");
        let mut different_server = first.clone();
        different_server.id = McpServerId::new();
        assert_ne!(
            compute_launch_spec_digest(&first).unwrap(),
            compute_launch_spec_digest(&different_server).unwrap()
        );

        let mut joined_argument = first.clone();
        let McpTransportConfig::Stdio(stdio) = &mut joined_argument.transport else {
            panic!("test config must use stdio");
        };
        stdio.arguments = vec!["--mode stdio".to_string(), String::new()];
        assert_ne!(
            compute_launch_spec_digest(&first).unwrap(),
            compute_launch_spec_digest(&joined_argument).unwrap()
        );

        let mut without_empty = first.clone();
        let McpTransportConfig::Stdio(stdio) = &mut without_empty.transport else {
            panic!("test config must use stdio");
        };
        stdio.arguments.pop();
        assert_ne!(
            compute_launch_spec_digest(&first).unwrap(),
            compute_launch_spec_digest(&without_empty).unwrap()
        );
    }

    #[test]
    fn startup_reconciles_digest_corruption_to_disabled_untrusted_state() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let added = registry.add(config(id, "recover")).unwrap();
        drop(registry);

        let inspection = Connection::open(&path).unwrap();
        inspection
            .execute(
                "UPDATE mcp_registry_servers
                 SET config_digest = ?2, enabled = 1, trust = 'user_approved'
                 WHERE server_id = ?1",
                params![id.to_string(), "f".repeat(64)],
            )
            .unwrap();
        drop(inspection);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        let recovered = reopened.get_persisted(id).unwrap().unwrap();
        assert!(!recovered.entry.config.enabled);
        assert_eq!(recovered.entry.config.trust, McpTrustLevel::Untrusted);
        assert!(recovered.entry.revision > added.revision);
        assert_ne!(recovered.entry.config_epoch, added.config_epoch);
        assert_eq!(
            recovered.safe_error_code.as_deref(),
            Some(SAFE_ERROR_RECONCILED)
        );
        assert!(recovered.launch_authorization.is_none());
    }

    #[test]
    fn startup_quarantines_one_bad_storage_class_without_hiding_valid_servers() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let bad_id = McpServerId::new();
        let good_id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        registry.add(config(bad_id, "bad storage class")).unwrap();
        let expected_good = registry.add(config(good_id, "valid neighbor")).unwrap();
        drop(registry);

        let blob_canary = vec![0xff, 0x00, 0x80, 0x41];
        let corruption = Connection::open(&path).unwrap();
        corruption
            .execute(
                "UPDATE mcp_registry_servers
                 SET display_name = ?2
                 WHERE server_id = ?1",
                params![bad_id.to_string(), &blob_canary],
            )
            .unwrap();
        drop(corruption);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        assert_eq!(reopened.get(good_id).unwrap(), Some(expected_good.clone()));
        assert!(reopened.get(bad_id).unwrap().is_none());
        assert_eq!(reopened.list().unwrap(), vec![expected_good.clone()]);
        assert_eq!(reopened.current_revision().unwrap(), 3);
        drop(reopened);

        let inspection = Connection::open(&path).unwrap();
        let quarantined = inspection
            .query_row(
                "SELECT display_name, record_state, enabled, trust,
                        safe_error_code, registry_revision
                 FROM mcp_registry_servers
                 WHERE server_id = ?1",
                [bad_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            quarantined,
            (
                blob_canary,
                RECORD_STATE_INVALID.to_string(),
                0,
                "untrusted".to_string(),
                SAFE_ERROR_INVALID.to_string(),
                3,
            )
        );
        drop(inspection);

        let reopened_again = SqliteMcpRegistry::open(&path).unwrap();
        assert_eq!(reopened_again.current_revision().unwrap(), 3);
        assert_eq!(reopened_again.get(good_id).unwrap(), Some(expected_good));
        assert!(reopened_again.get(bad_id).unwrap().is_none());
    }

    #[test]
    fn unknown_row_schema_is_quarantined_and_never_listed() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("registry.sqlite");
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        registry.add(config(id, "quarantine")).unwrap();
        drop(registry);

        let inspection = Connection::open(&path).unwrap();
        inspection
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        inspection
            .execute(
                "UPDATE mcp_registry_servers SET schema_version = 99 WHERE server_id = ?1",
                [id.to_string()],
            )
            .unwrap();
        drop(inspection);

        let reopened = SqliteMcpRegistry::open(&path).unwrap();
        assert!(reopened.get(id).unwrap().is_none());
        let inspection = Connection::open(&path).unwrap();
        let state = inspection
            .query_row(
                "SELECT record_state, enabled, trust, safe_error_code
                 FROM mcp_registry_servers WHERE server_id = ?1",
                [id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            state,
            (
                RECORD_STATE_INVALID.to_string(),
                0,
                "untrusted".to_string(),
                SAFE_ERROR_INVALID.to_string()
            )
        );
    }

    fn tokio_test_block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(future)
    }
}
