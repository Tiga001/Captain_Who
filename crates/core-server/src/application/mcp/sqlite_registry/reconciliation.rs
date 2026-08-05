//! Fail-closed startup reconciliation for persisted Registry rows.

use super::*;

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

pub(super) fn reconcile_startup(
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
