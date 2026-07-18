use crate::storage::now_ms;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillEnablementState {
    pub enabled: bool,
    pub generation: u64,
}

/// Loads only explicit overrides for the requested, already-validated Skill ids.
///
/// Callers apply the product default (`enabled = true`) when an id is absent.
/// Point reads share one transaction so the batch observes one SQLite snapshot
/// without scanning unrelated or stale overrides.
pub fn load_skill_enablement_overrides(
    connection: &mut Connection,
    skill_ids: &[String],
) -> rusqlite::Result<BTreeMap<String, bool>> {
    if skill_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    let requested = skill_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let transaction = connection.transaction()?;
    let mut statement = transaction
        .prepare("SELECT enabled FROM skill_enablement_overrides WHERE skill_id = ?1")?;
    let mut overrides = BTreeMap::new();
    for skill_id in requested {
        let enabled = statement
            .query_row(params![skill_id], |row| row.get::<_, bool>(0))
            .optional()?;
        if let Some(enabled) = enabled {
            let skill_id = skill_id.to_string();
            overrides.insert(skill_id, enabled);
        }
    }
    drop(statement);
    transaction.commit()?;
    Ok(overrides)
}

/// Loads durable state tokens for requested Skill ids. Missing overrides use
/// the product default (`enabled = true`) at generation zero.
pub fn load_skill_enablement_states(
    connection: &mut Connection,
    skill_ids: &[String],
) -> rusqlite::Result<BTreeMap<String, SkillEnablementState>> {
    let requested = skill_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let transaction = connection.transaction()?;
    let mut statement = transaction.prepare(
        "SELECT enabled, generation FROM skill_enablement_overrides WHERE skill_id = ?1",
    )?;
    let mut states = BTreeMap::new();
    for skill_id in requested {
        let stored = statement
            .query_row(params![skill_id], |row| {
                Ok((row.get::<_, bool>(0)?, row.get::<_, i64>(1)?))
            })
            .optional()?;
        let state = match stored {
            Some((enabled, generation)) => SkillEnablementState {
                enabled,
                generation: u64::try_from(generation)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, generation))?,
            },
            None => SkillEnablementState {
                enabled: true,
                generation: 0,
            },
        };
        states.insert(skill_id.to_string(), state);
    }
    drop(statement);
    transaction.commit()?;
    Ok(states)
}

/// Persists one explicit override and reports whether durable state changed.
pub fn set_skill_enablement_override(
    connection: &Connection,
    skill_id: &str,
    enabled: bool,
) -> rusqlite::Result<bool> {
    let changed = connection.execute(
        "
        INSERT INTO skill_enablement_overrides (skill_id, enabled, generation, updated_at)
        VALUES (?1, ?2, CASE WHEN ?2 THEN 0 ELSE 1 END, ?3)
        ON CONFLICT(skill_id) DO UPDATE SET
            enabled = excluded.enabled,
            generation = skill_enablement_overrides.generation + 1,
            updated_at = excluded.updated_at
        WHERE skill_enablement_overrides.enabled != excluded.enabled
        ",
        params![skill_id, enabled, now_ms()],
    )?;
    Ok(changed > 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillEnablementCompareAndSetOutcome {
    Updated { generation: u64 },
    AlreadyCurrent { generation: u64 },
    Conflict,
}

/// Atomically compares effective enablement (where absence means `true`) and
/// persists the requested target. The immediate transaction prevents two
/// cooperating writers from both committing from the same observed state.
pub fn compare_and_set_skill_enablement(
    connection: &mut Connection,
    skill_id: &str,
    expected_enabled: bool,
    expected_generation: u64,
    target: bool,
) -> rusqlite::Result<SkillEnablementCompareAndSetOutcome> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stored = transaction
        .query_row(
            "SELECT enabled, generation FROM skill_enablement_overrides WHERE skill_id = ?1",
            params![skill_id],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let (actual_enabled, actual_generation) = match stored {
        Some((enabled, generation)) => (
            enabled,
            u64::try_from(generation)
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, generation))?,
        ),
        None => (true, 0),
    };

    let outcome = if actual_enabled == target {
        SkillEnablementCompareAndSetOutcome::AlreadyCurrent {
            generation: actual_generation,
        }
    } else if actual_enabled != expected_enabled || actual_generation != expected_generation {
        SkillEnablementCompareAndSetOutcome::Conflict
    } else {
        let generation = actual_generation
            .checked_add(1)
            .ok_or_else(|| rusqlite::Error::IntegralValueOutOfRange(1, i64::MAX))?;
        let stored_generation = i64::try_from(generation)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, i64::MAX))?;
        transaction.execute(
            "
            INSERT INTO skill_enablement_overrides (skill_id, enabled, generation, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(skill_id) DO UPDATE SET
                enabled = excluded.enabled,
                generation = excluded.generation,
                updated_at = excluded.updated_at
            ",
            params![skill_id, target, stored_generation, now_ms()],
        )?;
        SkillEnablementCompareAndSetOutcome::Updated { generation }
    };
    transaction.commit()?;
    Ok(outcome)
}

/// Deletes one explicit override and reports whether a row existed.
pub fn delete_skill_enablement_override(
    connection: &Connection,
    skill_id: &str,
) -> rusqlite::Result<bool> {
    Ok(connection.execute(
        "DELETE FROM skill_enablement_overrides WHERE skill_id = ?1",
        params![skill_id],
    )? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    #[test]
    fn explicit_overrides_round_trip_idempotently_and_delete_cleanly() {
        let mut connection = Connection::open_in_memory().expect("test database should open");
        migrations::run_migrations(&connection).expect("test database should migrate");
        let first = "bundled:application:auditor".to_string();
        let second = "installed:user:01234567-89ab-4def-8123-456789abcdef".to_string();
        let missing = "bundled:application:missing".to_string();

        assert!(set_skill_enablement_override(&connection, &first, false).unwrap());
        assert!(!set_skill_enablement_override(&connection, &first, false).unwrap());
        assert!(set_skill_enablement_override(&connection, &second, true).unwrap());

        let overrides = load_skill_enablement_overrides(
            &mut connection,
            &[missing, second.clone(), first.clone(), first.clone()],
        )
        .unwrap();
        assert_eq!(overrides.len(), 2);
        assert_eq!(overrides.get(&first), Some(&false));
        assert_eq!(overrides.get(&second), Some(&true));

        assert!(delete_skill_enablement_override(&connection, &first).unwrap());
        assert!(!delete_skill_enablement_override(&connection, &first).unwrap());
        assert!(load_skill_enablement_overrides(&mut connection, &[first])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn schema_rejects_empty_oversized_and_non_boolean_rows() {
        let connection = Connection::open_in_memory().expect("test database should open");
        migrations::run_migrations(&connection).expect("test database should migrate");

        assert!(connection
            .execute(
                "INSERT INTO skill_enablement_overrides (skill_id, enabled, generation, updated_at) VALUES ('', 1, 0, 1)",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO skill_enablement_overrides (skill_id, enabled, generation, updated_at) VALUES (?1, 1, 0, 1)",
                params!["x".repeat(16 * 1024 + 1)],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO skill_enablement_overrides (skill_id, enabled, generation, updated_at) VALUES ('valid:id:value', 2, 0, 1)",
                [],
            )
            .is_err());
    }

    #[test]
    fn compare_and_set_is_atomic_idempotent_and_detects_stale_state() {
        let mut connection = Connection::open_in_memory().expect("test database should open");
        migrations::run_migrations(&connection).expect("test database should migrate");
        let skill_id = "bundled:application:auditor";

        assert_eq!(
            compare_and_set_skill_enablement(&mut connection, skill_id, true, 0, false).unwrap(),
            SkillEnablementCompareAndSetOutcome::Updated { generation: 1 }
        );
        assert_eq!(
            compare_and_set_skill_enablement(&mut connection, skill_id, true, 0, false).unwrap(),
            SkillEnablementCompareAndSetOutcome::AlreadyCurrent { generation: 1 }
        );
        assert_eq!(
            compare_and_set_skill_enablement(&mut connection, skill_id, true, 0, true).unwrap(),
            SkillEnablementCompareAndSetOutcome::Conflict
        );
        assert_eq!(
            compare_and_set_skill_enablement(&mut connection, skill_id, false, 1, true).unwrap(),
            SkillEnablementCompareAndSetOutcome::Updated { generation: 2 }
        );
        assert_eq!(
            compare_and_set_skill_enablement(&mut connection, skill_id, true, 0, false).unwrap(),
            SkillEnablementCompareAndSetOutcome::Conflict,
            "generation rejects enabled-value ABA"
        );
        assert_eq!(
            load_skill_enablement_overrides(&mut connection, &[skill_id.to_string()],)
                .unwrap()
                .get(skill_id),
            Some(&true)
        );
    }
}
