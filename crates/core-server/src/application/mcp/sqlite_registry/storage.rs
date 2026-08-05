//! SQLite row encoding, decoding, and revision bookkeeping.

use super::*;

pub(super) fn insert_row(
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
pub(super) fn update_row(
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

pub(super) fn load_record(
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

pub(super) fn list_records(
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

pub(super) fn scan_raw_records(
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

pub(super) struct ScannedRawRegistryRow {
    pub(super) row_id: i64,
    pub(super) revision_candidate: Option<u64>,
    pub(super) already_invalid: bool,
    pub(super) record: Option<RawRegistryRecord>,
}

#[derive(Clone)]
pub(super) struct RawRegistryRecord {
    pub(super) schema_version: i64,
    pub(super) server_id: String,
    pub(super) display_name: String,
    pub(super) scope_kind: String,
    pub(super) source_kind: String,
    pub(super) transport_kind: String,
    pub(super) executable: String,
    pub(super) arguments_json: String,
    pub(super) cwd: String,
    pub(super) enabled: i64,
    pub(super) trust: String,
    pub(super) approval_mode: String,
    pub(super) connect_timeout_ms: i64,
    pub(super) request_timeout_ms: i64,
    pub(super) shutdown_timeout_ms: i64,
    pub(super) config_digest: String,
    pub(super) config_epoch: String,
    pub(super) registry_revision: i64,
    pub(super) launch_spec_digest: String,
    pub(super) authorized_launch_spec_digest: Option<String>,
    pub(super) authorized_config_epoch: Option<String>,
    pub(super) authorized_config_digest: Option<String>,
    pub(super) authorization_format_version: Option<i64>,
    pub(super) authorization_policy_version: Option<i64>,
    pub(super) authorized_at: Option<i64>,
    pub(super) record_state: String,
    pub(super) safe_error_code: Option<String>,
    pub(super) created_at: i64,
    pub(super) updated_at: i64,
    pub(super) model_namespace: Option<String>,
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

pub(super) fn decode_record(
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

pub(super) fn config_from_raw(
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

pub(super) fn registry_change(
    kind: McpRegistryChangeKind,
    entry: &McpRegistryEntry,
) -> McpRegistryChange {
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

pub(super) fn next_revision(
    transaction: &Transaction<'_>,
) -> Result<u64, McpRegistryPersistenceError> {
    let current = read_revision(transaction)?;
    let next = current
        .checked_add(1)
        .filter(|value| *value <= MAX_WIRE_SAFE_INTEGER)
        .ok_or(McpRegistryPersistenceError::RevisionExhausted)?;
    write_revision(transaction, next)?;
    Ok(next)
}

pub(super) fn read_revision(connection: &Connection) -> Result<u64, McpRegistryPersistenceError> {
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

pub(super) fn write_revision(
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

pub(super) fn quarantine_record_by_rowid(
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

pub(super) fn sqlite_revision(value: u64) -> Result<i64, McpRegistryPersistenceError> {
    if value > MAX_WIRE_SAFE_INTEGER {
        return Err(McpRegistryPersistenceError::RevisionExhausted);
    }
    i64::try_from(value).map_err(|_| McpRegistryPersistenceError::RevisionExhausted)
}

pub(super) fn now_ms() -> Result<i64, McpRegistryPersistenceError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
        .as_millis();
    i64::try_from(millis).map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

pub(super) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
