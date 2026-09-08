use super::storage_error;
use crate::{AgentCollaborationSettings, AgentCollaborationSettingsUpdate};
use rusqlite::{params, Connection, TransactionBehavior};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub(crate) fn load_settings(connection: &Connection) -> Result<AgentCollaborationSettings, String> {
    connection.query_row(
        "SELECT enabled, revision, updated_at FROM agent_collaboration_settings WHERE singleton = 1",
        [],
        |row| Ok(AgentCollaborationSettings { enabled: row.get(0)?, revision: row.get(1)?, updated_at: row.get(2)? }),
    ).map_err(storage_error)
}

pub(crate) fn update_settings(
    connection: &mut Connection,
    input: &AgentCollaborationSettingsUpdate,
    now: i64,
) -> Result<AgentCollaborationSettings, String> {
    if input.expected_revision == 0
        || input.expected_revision >= MAX_SAFE_INTEGER
        || now < 0
        || now as u64 > MAX_SAFE_INTEGER
    {
        return Err("Invalid collaboration settings revision or timestamp".into());
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let previous = load_settings(&transaction)?;
    if previous.revision != input.expected_revision {
        return Err("Collaboration settings changed; refresh and retry".into());
    }
    let next = AgentCollaborationSettings {
        enabled: input.enabled,
        revision: previous.revision + 1,
        updated_at: now.max(previous.updated_at),
    };
    transaction.execute("UPDATE agent_collaboration_settings SET enabled = ?1, revision = ?2, updated_at = ?3 WHERE singleton = 1", params![next.enabled, next.revision, next.updated_at]).map_err(storage_error)?;
    transaction.commit().map_err(storage_error)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::run_migrations;

    #[test]
    fn settings_default_enabled_and_reject_stale_or_unsafe_updates() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        assert_eq!(
            load_settings(&connection).unwrap(),
            AgentCollaborationSettings::default()
        );
        let change = AgentCollaborationSettingsUpdate {
            enabled: false,
            expected_revision: 1,
        };
        let updated = update_settings(&mut connection, &change, 123).unwrap();
        assert_eq!(
            (updated.enabled, updated.revision, updated.updated_at),
            (false, 2, 123)
        );
        assert!(update_settings(&mut connection, &change, 124).is_err());
        for revision in [0, MAX_SAFE_INTEGER, u64::MAX] {
            assert!(update_settings(
                &mut connection,
                &AgentCollaborationSettingsUpdate {
                    enabled: true,
                    expected_revision: revision
                },
                124
            )
            .is_err());
        }
        assert_eq!(load_settings(&connection).unwrap(), updated);
        let later = update_settings(
            &mut connection,
            &AgentCollaborationSettingsUpdate {
                enabled: true,
                expected_revision: 2,
            },
            1,
        )
        .unwrap();
        assert_eq!(
            (later.enabled, later.revision, later.updated_at),
            (true, 3, 123)
        );
    }
}
