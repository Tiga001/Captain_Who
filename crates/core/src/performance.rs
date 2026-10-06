//! Opt-in, bounded scalar diagnostics. Never pass IDs, paths, request bodies or errors as labels.
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_SERIES: usize = 128;

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("CAPTAIN_PERFORMANCE_DIAGNOSTICS").as_deref() == Ok("1"))
}

#[derive(Default, Debug)]
struct Sample {
    count: u64,
    total_us: u64,
    max_us: u64,
    bytes: u64,
    max_bytes: u64,
}

#[derive(Default)]
struct Window {
    samples: BTreeMap<(&'static str, &'static str), Sample>,
}

impl Window {
    fn record(&mut self, stage: &'static str, operation: &'static str, us: u64, bytes: u64) {
        let key = (stage, operation);
        // Even an accidental proliferation of call sites cannot create an unbounded registry.
        if self.samples.len() >= MAX_SERIES && !self.samples.contains_key(&key) {
            return;
        }
        let sample = self.samples.entry(key).or_default();
        sample.count = sample.count.saturating_add(1);
        sample.total_us = sample.total_us.saturating_add(us);
        sample.max_us = sample.max_us.max(us);
        sample.bytes = sample.bytes.saturating_add(bytes);
        sample.max_bytes = sample.max_bytes.max(bytes);
    }
}

fn window() -> &'static Mutex<Window> {
    static WINDOW: OnceLock<Mutex<Window>> = OnceLock::new();
    WINDOW.get_or_init(|| Mutex::new(Window::default()))
}

pub fn record(stage: &'static str, operation: &'static str, elapsed: Duration, bytes: usize) {
    if !enabled() {
        return;
    }
    // Producers may hold service/SQLite locks. Only update counters here; the transport owner
    // flushes them periodically, outside business callbacks and locks.
    let mut window = window().lock().unwrap_or_else(|error| error.into_inner());
    window.record(
        stage,
        operation,
        elapsed.as_micros().min(u64::MAX as u128) as u64,
        bytes as u64,
    );
}

fn emit(samples: BTreeMap<(&'static str, &'static str), Sample>) {
    // Never format or write logs while holding the diagnostics mutex.
    for ((stage, operation), sample) in samples {
        tracing::info!(target: "core_performance", stage, operation,
            count = sample.count, total_us = sample.total_us, max_us = sample.max_us,
            bytes = sample.bytes, max_bytes = sample.max_bytes, "performance window");
    }
}

pub fn flush() {
    if !enabled() {
        return;
    }
    let samples = {
        let mut window = window().lock().unwrap_or_else(|error| error.into_inner());
        std::mem::take(&mut window.samples)
    };
    emit(samples);
}

/// Scope timing has no clock/registry cost while diagnostics are disabled.
pub struct Span {
    start: Option<Instant>,
    stage: &'static str,
    operation: &'static str,
}

impl Span {
    pub fn new(stage: &'static str, operation: &'static str) -> Self {
        Self {
            start: enabled().then(Instant::now),
            stage,
            operation,
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            record(self.stage, self.operation, start.elapsed(), 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_aggregate_scalars_without_retaining_individual_samples() {
        let mut window = Window::default();
        for _ in 0..10_000 {
            window.record("rpc", "read", 12, 80);
        }
        window.record("rpc", "read", 50, 120);
        assert_eq!(window.samples.len(), 1);
        let sample = &window.samples[&("rpc", "read")];
        assert_eq!(sample.count, 10_001);
        assert_eq!(sample.total_us, 120_050);
        assert_eq!(sample.max_us, 50);
        assert_eq!(sample.bytes, 800_120);
        assert_eq!(sample.max_bytes, 120);
        let completed = std::mem::take(&mut window.samples);
        assert_eq!(completed.len(), 1);
        assert!(window.samples.is_empty());
    }

    #[test]
    fn registry_is_bounded_and_counters_saturate() {
        let mut window = Window::default();
        for index in 0..MAX_SERIES {
            // Test-only leaked labels model distinct static call sites, never user input.
            let label = Box::leak(format!("series{index}").into_boxed_str());
            window.record("test", label, 0, 0);
        }
        window.record("overflow", "discarded", 1, 1);
        assert_eq!(window.samples.len(), MAX_SERIES);
        assert!(!window.samples.contains_key(&("overflow", "discarded")));
        let sample = window.samples.get_mut(&("test", "series0")).unwrap();
        sample.count = u64::MAX;
        sample.total_us = u64::MAX;
        sample.bytes = u64::MAX;
        window.record("test", "series0", 10, 10);
        let sample = &window.samples[&("test", "series0")];
        assert_eq!(sample.count, u64::MAX);
        assert_eq!(sample.total_us, u64::MAX);
        assert_eq!(sample.bytes, u64::MAX);
        assert_eq!(sample.max_us, 10);
    }
}
