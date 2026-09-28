//! Test-only counters for actual durable-history decoding, not estimated end-to-end speedups.
use std::cell::RefCell;

#[derive(Debug, Default, Clone)]
pub(crate) struct TraceReadMetrics {
    pub trace_loads: usize,
    pub trace_bytes: usize,
    pub context_loads: usize,
    pub context_bytes: usize,
    pub decompressions: usize,
    pub lock_wait_ns: Vec<u64>,
    pub transaction_ns: Vec<u64>,
}

thread_local! { static METRICS: RefCell<Option<TraceReadMetrics>> = const { RefCell::new(None) }; }

pub(crate) fn start() {
    METRICS.with(|value| *value.borrow_mut() = Some(TraceReadMetrics::default()));
}
pub(crate) fn finish() -> TraceReadMetrics {
    METRICS.with(|value| value.borrow_mut().take().unwrap_or_default())
}
pub(crate) fn trace(bytes: usize) {
    METRICS.with(|value| {
        if let Some(metrics) = value.borrow_mut().as_mut() {
            metrics.trace_loads += 1;
            metrics.trace_bytes += bytes;
        }
    });
}
pub(crate) fn context_load() {
    METRICS.with(|value| {
        if let Some(metrics) = value.borrow_mut().as_mut() {
            metrics.context_loads += 1;
        }
    });
}
pub(crate) fn decompressed(bytes: usize) {
    METRICS.with(|value| {
        if let Some(metrics) = value.borrow_mut().as_mut() {
            metrics.decompressions += 1;
            metrics.context_bytes += bytes;
        }
    });
}
pub(crate) fn lock_wait(elapsed: std::time::Duration) {
    METRICS.with(|value| {
        if let Some(metrics) = value.borrow_mut().as_mut() {
            metrics.lock_wait_ns.push(elapsed.as_nanos() as u64);
        }
    });
}

pub(crate) fn transaction(elapsed: std::time::Duration) {
    METRICS.with(|value| {
        if let Some(metrics) = value.borrow_mut().as_mut() {
            metrics.transaction_ns.push(elapsed.as_nanos() as u64);
        }
    });
}
