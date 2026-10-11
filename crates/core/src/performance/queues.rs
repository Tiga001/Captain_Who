//! Bounded opt-in diagnostic ownership. Entries do not control application admission or order.
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const MAX_QUEUES: usize = 32;
const MAX_ENTRIES: usize = 512;
type Key = (&'static str, &'static str);

#[derive(Default)]
struct Queue {
    entries: BTreeMap<u64, (Instant, usize)>,
    bytes: usize,
    peak_count: usize,
    peak_bytes: usize,
    omitted: usize,
}

#[derive(Default)]
struct Registry {
    queues: BTreeMap<Key, Queue>,
    next_id: u64,
    entries: usize,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

#[derive(Debug, serde::Serialize)]
pub struct QueueMeasurement {
    pub stage: &'static str,
    pub operation: &'static str,
    pub count: usize,
    /// Only known wire/body bytes, not an estimate of heap ownership. Zero may mean unmeasured.
    pub bytes: usize,
    pub oldest_us: u64,
    pub peak_count: usize,
    pub peak_bytes: usize,
    pub omitted: usize,
}

pub struct QueueEntry {
    key: Key,
    id: u64,
}

impl Registry {
    fn insert(&mut self, key: Key, bytes: usize, now: Instant) -> Option<QueueEntry> {
        if self.queues.len() >= MAX_QUEUES && !self.queues.contains_key(&key) {
            return None;
        }
        let queue = self.queues.entry(key).or_default();
        if self.entries >= MAX_ENTRIES || self.next_id == u64::MAX {
            queue.omitted = queue.omitted.saturating_add(1);
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries += 1;
        queue.entries.insert(id, (now, bytes));
        queue.bytes = queue.bytes.saturating_add(bytes);
        queue.peak_count = queue.peak_count.max(queue.entries.len());
        queue.peak_bytes = queue.peak_bytes.max(queue.bytes);
        Some(QueueEntry { key, id })
    }

    fn remove(&mut self, entry: &QueueEntry) {
        if let Some(queue) = self.queues.get_mut(&entry.key) {
            if let Some((_, bytes)) = queue.entries.remove(&entry.id) {
                queue.bytes = queue.bytes.saturating_sub(bytes);
                self.entries -= 1;
            }
        }
    }

    fn resize(&mut self, entry: &QueueEntry, bytes: usize) {
        if let Some(queue) = self.queues.get_mut(&entry.key) {
            if let Some((_, previous_bytes)) = queue.entries.get_mut(&entry.id) {
                queue.bytes = queue
                    .bytes
                    .saturating_sub(*previous_bytes)
                    .saturating_add(bytes);
                *previous_bytes = bytes;
                queue.peak_bytes = queue.peak_bytes.max(queue.bytes);
            }
        }
    }

    fn snapshot(&mut self, now: Instant) -> Vec<QueueMeasurement> {
        self.queues
            .iter_mut()
            .map(|(&(stage, operation), queue)| {
                let measurement = QueueMeasurement {
                    stage,
                    operation,
                    count: queue.entries.len(),
                    bytes: queue.bytes,
                    oldest_us: queue
                        .entries
                        .first_key_value()
                        .map(|(_, (created, _))| {
                            now.saturating_duration_since(*created)
                                .as_micros()
                                .min(u64::MAX as u128) as u64
                        })
                        .unwrap_or(0),
                    peak_count: queue.peak_count,
                    peak_bytes: queue.peak_bytes,
                    omitted: std::mem::take(&mut queue.omitted),
                };
                queue.peak_count = queue.entries.len();
                queue.peak_bytes = queue.bytes;
                measurement
            })
            .collect()
    }
}

/// A static stage/operation pair only; never use a request/conversation ID as a key.
pub fn track_queue(
    stage: &'static str,
    operation: &'static str,
    bytes: usize,
) -> Option<QueueEntry> {
    if !super::enabled() {
        return None;
    }
    registry()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert((stage, operation), bytes, Instant::now())
}

impl Drop for QueueEntry {
    fn drop(&mut self) {
        registry()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(self);
    }
}

impl QueueEntry {
    /// Update retained-byte accounting without changing queue identity, age or count.
    pub fn resize_bytes(&mut self, bytes: usize) {
        registry()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .resize(self, bytes);
    }
}

/// Resets window peaks, retaining pending entries/age across windows. No application effects.
pub fn take_queue_snapshot() -> Vec<QueueMeasurement> {
    if !super::enabled() {
        return Vec::new();
    }
    registry()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .snapshot(Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_retains_age_and_count_and_reports_the_peak_bytes() {
        let now = Instant::now();
        let mut registry = Registry::default();
        let entry = registry
            .insert(("blocking", "completion"), 10, now)
            .unwrap();
        registry.resize(&entry, 90);
        registry.resize(&entry, 20);
        let snapshot = registry.snapshot(now + std::time::Duration::from_millis(40));
        assert_eq!(
            (snapshot[0].count, snapshot[0].bytes, snapshot[0].oldest_us),
            (1, 20, 40_000)
        );
        assert_eq!((snapshot[0].peak_count, snapshot[0].peak_bytes), (1, 90));
        registry.remove(&entry);
        assert_eq!(registry.snapshot(now)[0].bytes, 0);
        std::mem::forget(entry);
    }

    #[test]
    fn pending_age_survives_window_reset_and_removed_entries_release_bytes() {
        let now = Instant::now();
        let mut registry = Registry::default();
        let a = registry.insert(("rpc", "read"), 30, now).unwrap();
        let b = registry.insert(("rpc", "read"), 70, now).unwrap();
        let first = registry.snapshot(now + std::time::Duration::from_millis(40));
        assert_eq!(
            (first[0].count, first[0].bytes, first[0].oldest_us),
            (2, 100, 40_000)
        );
        registry.remove(&a);
        registry.remove(&b);
        let drained = registry.snapshot(now);
        assert_eq!(
            (drained[0].count, drained[0].bytes, drained[0].oldest_us),
            (0, 0, 0)
        );
        assert_eq!((drained[0].peak_count, drained[0].peak_bytes), (2, 100));
        assert_eq!(registry.snapshot(now)[0].peak_count, 0);
        // Local test registry entries must not drop into the unrelated process-global registry.
        std::mem::forget(a);
        std::mem::forget(b);
    }

    #[test]
    fn diagnostics_have_a_hard_entry_budget_without_affecting_work() {
        let mut registry = Registry::default();
        for _ in 0..MAX_ENTRIES {
            std::mem::forget(registry.insert(("rpc", "read"), 1, Instant::now()).unwrap());
        }
        assert!(registry
            .insert(("rpc", "read"), 1, Instant::now())
            .is_none());
        let snapshot = registry.snapshot(Instant::now());
        assert_eq!(snapshot[0].count, MAX_ENTRIES);
        assert_eq!(snapshot[0].omitted, 1);
    }
}
