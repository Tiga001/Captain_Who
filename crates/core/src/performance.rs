//! Opt-in, bounded scalar diagnostics. Never pass IDs, paths, request bodies or errors as labels.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
mod queues;
pub use queues::{take_queue_snapshot, track_queue, QueueEntry, QueueMeasurement};

// The audited RPC/SQLite/Turn vocabulary exceeds 128 pairs. This bounds diagnostics only,
// never application queues, and leaves room for all current static call sites.
const MAX_SERIES: usize = 256;
// Eight buckets per power of two (exact through 15us), bounded independently of traffic.
const HISTOGRAM_BUCKETS: usize = 496;
static REQUEST_TRACES: AtomicUsize = AtomicUsize::new(0);

/// At most 32 numeric request-ID traces per diagnostic window. No user identifiers or payloads.
/// IDs correlate Host/Core records; durations remain local to each process's monotonic clock.
pub fn trace_request(
    id: i64,
    lane: &'static str,
    queue: Duration,
    processing: Duration,
    failed: bool,
) {
    if !enabled() {
        return;
    }
    let mut count = REQUEST_TRACES.load(Ordering::Relaxed);
    loop {
        if count >= 32 {
            return;
        }
        match REQUEST_TRACES.compare_exchange_weak(
            count,
            count + 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(current) => count = current,
        }
    }
    tracing::info!(target: "core_performance", request_id = id, lane,
        queue_us = queue.as_micros().min(u64::MAX as u128) as u64,
        processing_us = processing.as_micros().min(u64::MAX as u128) as u64,
        failed, "performance request trace");
}

#[derive(Debug)]
struct Histogram([u64; HISTOGRAM_BUCKETS]);

impl Default for Histogram {
    fn default() -> Self {
        Self([0; HISTOGRAM_BUCKETS])
    }
}

impl Histogram {
    fn record(&mut self, value: u64) {
        let index = if value < 8 {
            value as usize
        } else {
            let exponent = 63 - value.leading_zeros() as usize;
            (exponent - 3) * 8 + (value >> (exponent - 3)) as usize
        };
        self.0[index] = self.0[index].saturating_add(1);
    }

    fn upper_quantile(&self, count: u64, percentile: u64, max: u64) -> u64 {
        if count == 0 {
            return 0;
        }
        let rank = (u128::from(count) * u128::from(percentile)).div_ceil(100);
        let mut seen = 0_u128;
        for (index, count) in self.0.iter().enumerate() {
            seen += u128::from(*count);
            if seen >= rank {
                let upper = if index < 8 {
                    index as u128
                } else {
                    (((9 + index % 8) as u128) << (index / 8 - 1)) - 1
                };
                return upper.min(u128::from(max)) as u64;
            }
        }
        max
    }
}

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
    histogram: Histogram,
}

/// Window-local summaries. Quantiles are histogram bucket upper bounds, never exact samples.
/// Small windows still expose their count so callers cannot mistake an unstable p99 for an SLO.
#[derive(Debug, serde::Serialize)]
pub struct Measurement {
    pub stage: &'static str,
    pub operation: &'static str,
    pub count: u64,
    pub total_us: u64,
    pub max_us: u64,
    pub bytes: u64,
    pub max_bytes: u64,
    pub p50_upper_us: u64,
    pub p95_upper_us: u64,
    pub p99_upper_us: u64,
}

#[derive(Default)]
struct Window {
    samples: BTreeMap<(&'static str, &'static str), Sample>,
    omitted_samples: u64,
}

impl Window {
    fn record(&mut self, stage: &'static str, operation: &'static str, us: u64, bytes: u64) {
        let key = (stage, operation);
        // Even an accidental proliferation of call sites cannot create an unbounded registry.
        if self.samples.len() >= MAX_SERIES && !self.samples.contains_key(&key) {
            self.omitted_samples = self.omitted_samples.saturating_add(1);
            return;
        }
        let sample = self.samples.entry(key).or_default();
        sample.count = sample.count.saturating_add(1);
        sample.total_us = sample.total_us.saturating_add(us);
        sample.max_us = sample.max_us.max(us);
        sample.bytes = sample.bytes.saturating_add(bytes);
        sample.max_bytes = sample.max_bytes.max(bytes);
        sample.histogram.record(us);
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

fn summarize(samples: BTreeMap<(&'static str, &'static str), Sample>) -> Vec<Measurement> {
    samples
        .into_iter()
        .map(|((stage, operation), sample)| Measurement {
            stage,
            operation,
            count: sample.count,
            total_us: sample.total_us,
            max_us: sample.max_us,
            bytes: sample.bytes,
            max_bytes: sample.max_bytes,
            p50_upper_us: sample
                .histogram
                .upper_quantile(sample.count, 50, sample.max_us),
            p95_upper_us: sample
                .histogram
                .upper_quantile(sample.count, 95, sample.max_us),
            p99_upper_us: sample
                .histogram
                .upper_quantile(sample.count, 99, sample.max_us),
        })
        .collect()
}

/// Drains only the bounded diagnostic window; does not read or change application state.
/// The normal transport flusher and benchmark collector must not run concurrently if a complete
/// per-case window is required. Disabled diagnostics return no measurements.
pub fn take_snapshot() -> Vec<Measurement> {
    if !enabled() {
        return Vec::new();
    }
    REQUEST_TRACES.store(0, Ordering::Relaxed);
    let (samples, omitted_samples) = {
        let mut window = window().lock().unwrap_or_else(|error| error.into_inner());
        (
            std::mem::take(&mut window.samples),
            std::mem::take(&mut window.omitted_samples),
        )
    };
    let mut measurements = summarize(samples);
    if omitted_samples > 0 {
        measurements.push(Measurement {
            stage: "diagnostics.omitted",
            operation: "series_budget",
            count: omitted_samples,
            total_us: 0,
            max_us: 0,
            bytes: 0,
            max_bytes: 0,
            p50_upper_us: 0,
            p95_upper_us: 0,
            p99_upper_us: 0,
        });
    }
    measurements
}

fn emit(samples: Vec<Measurement>) {
    // Never format or write logs while holding the diagnostics mutex.
    for sample in samples {
        tracing::info!(target: "core_performance", stage = sample.stage, operation = sample.operation,
            count = sample.count, total_us = sample.total_us, max_us = sample.max_us,
            bytes = sample.bytes, max_bytes = sample.max_bytes,
            p50_upper_us = sample.p50_upper_us, p95_upper_us = sample.p95_upper_us,
            p99_upper_us = sample.p99_upper_us, "performance window");
    }
}

pub fn flush() {
    if !enabled() {
        return;
    }
    emit(take_snapshot());
    for queue in take_queue_snapshot() {
        tracing::info!(target: "core_performance", stage = queue.stage, operation = queue.operation,
            count = queue.count, bytes = queue.bytes, oldest_us = queue.oldest_us,
            peak_count = queue.peak_count, peak_bytes = queue.peak_bytes,
            omitted = queue.omitted, "performance queue window");
    }
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
    fn histogram_quantiles_bound_samples_and_retain_the_tail() {
        let mut histogram = Histogram::default();
        for sample in 0..10_000 {
            histogram.record(sample);
        }
        for (percentile, exact) in [(50, 4_999), (95, 9_499), (99, 9_899)] {
            let upper = histogram.upper_quantile(10_000, percentile, 9_999);
            assert!(upper >= exact && upper <= exact + exact / 8 + 1);
        }
        let mut tail = Histogram::default();
        tail.record(u64::MAX);
        assert_eq!(tail.upper_quantile(1, 99, u64::MAX), u64::MAX);
        assert_eq!(tail.upper_quantile(0, 99, 0), 0);
        for value in 0..16 {
            let mut exact = Histogram::default();
            exact.record(value);
            assert_eq!(exact.upper_quantile(1, 95, value), value);
        }
    }

    #[test]
    fn summaries_reset_counts_and_report_upper_bound_quantiles() {
        let mut window = Window::default();
        for value in [2, 4, 8, 1_000_000] {
            window.record("test", "tail", value, 16);
        }
        let samples = summarize(std::mem::take(&mut window.samples));
        assert!(window.samples.is_empty());
        assert_eq!(samples[0].count, 4);
        assert_eq!(samples[0].p50_upper_us, 4);
        assert_eq!(samples[0].p99_upper_us, 1_000_000);
        assert_eq!(samples[0].bytes, 64);
    }

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
        assert_eq!(window.omitted_samples, 1);
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
