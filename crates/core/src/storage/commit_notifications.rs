//! Commit-aware, process-local wakeups for the durable event journals.
//!
//! SQLite's update hook is an invalidation hint, not evidence that a transaction committed. The
//! connection owner must call `publish_committed` after an operation's statements and transactions
//! have finished, including error paths. This also covers repository-managed transactions, raw
//! savepoints, and autocommit statements without changing their atomicity or return values.

use super::{StorageEventNotifications, StorageEventStream};
use rusqlite::{hooks::Action, Connection};
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::Arc;

const JOURNALS: [(&str, &str, StorageEventStream); 3] = [
    (
        "agent_collaboration_events",
        "SELECT COALESCE(MAX(global_sequence), 0) FROM agent_collaboration_events",
        StorageEventStream::AgentCollaboration,
    ),
    (
        "automation_events",
        "SELECT COALESCE(MAX(sequence), 0) FROM automation_events",
        StorageEventStream::Automation,
    ),
    (
        "notification_change_events",
        "SELECT COALESCE(MAX(sequence), 0) FROM notification_change_events",
        StorageEventStream::Notification,
    ),
];

#[derive(Clone)]
pub(super) struct CommitNotifications {
    inner: Arc<CommitNotificationState>,
}

struct CommitNotificationState {
    dirty: AtomicU8,
    published_heads: [AtomicI64; 3],
    notifications: StorageEventNotifications,
}

impl CommitNotifications {
    /// Install once on the exclusively owned writer, after schema migration and before accepting
    /// work. Read-pool connections deliberately do not install hooks or acquire another identity.
    pub(super) fn install(
        connection: &Connection,
        notifications: StorageEventNotifications,
    ) -> rusqlite::Result<Self> {
        let mut heads = [0; 3];
        for (index, (_, query, _)) in JOURNALS.iter().enumerate() {
            heads[index] = connection.query_row(query, [], |row| row.get(0))?;
        }
        let inner = Arc::new(CommitNotificationState {
            dirty: AtomicU8::new(0),
            published_heads: heads.map(AtomicI64::new),
            notifications,
        });
        let hook_state = Arc::clone(&inner);
        connection.update_hook(Some(
            move |action: Action, database: &str, table: &str, _row_id: i64| {
                if action != Action::SQLITE_INSERT || database != "main" {
                    return;
                }
                if let Some(index) = JOURNALS.iter().position(|(name, _, _)| *name == table) {
                    // No SQL or notification from inside a SQLite callback. Rolled-back inserts
                    // can set this bit; the completed-operation boundary verifies the journal.
                    hook_state.dirty.fetch_or(1 << index, Ordering::Relaxed);
                }
            },
        ))?;
        Ok(Self { inner })
    }

    /// Verify changed journal heads only when no explicit transaction/savepoint remains. The
    /// caller must exclusively own this connection and have finalized its SQL statements.
    ///
    /// A failed COMMIT may leave a transaction open. Keep its invalidations until the caller
    /// rolls back or retries successfully; never expose that connection's uncommitted rows.
    /// Do not clear dirty bits in a rollback hook: one operation may commit an earlier statement
    /// and roll back a later transaction, and the earlier committed event still needs a wakeup.
    ///
    /// Errors preserve the affected dirty bit for a subsequent attempt. Callers must report the
    /// diagnostic without relabeling an already committed mutation as failed. Durable journal
    /// catch-up and the existing bounded polling fallback remain authoritative.
    pub(super) fn publish_committed(&self, connection: &Connection) -> rusqlite::Result<()> {
        let dirty = self.inner.dirty.load(Ordering::Relaxed);
        if dirty == 0 || !connection.is_autocommit() {
            return Ok(());
        }
        for (index, (_, query, stream)) in JOURNALS.iter().enumerate() {
            let bit = 1 << index;
            if dirty & bit == 0 {
                continue;
            }
            let head: i64 = connection.query_row(query, [], |row| row.get(0))?;
            let previous = self.inner.published_heads[index].load(Ordering::Relaxed);
            if head > previous {
                self.inner.published_heads[index].store(head, Ordering::Relaxed);
                self.inner
                    .notifications
                    .sender(*stream)
                    .send_modify(|sequence| *sequence = sequence.saturating_add(1));
            }
            self.inner.dirty.fetch_and(!bit, Ordering::Relaxed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
