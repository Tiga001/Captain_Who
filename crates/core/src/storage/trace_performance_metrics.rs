//! Test-only counters for actual durable-history decoding, not estimated end-to-end speedups.
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

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

/// A caller-owned measurement scope, explicitly propagated across the storage writer boundary.
/// It is not global: concurrently running fixtures must never contribute to each other's counts.
#[derive(Clone)]
pub(crate) struct TraceMetricsContext(Arc<Mutex<TraceReadMetrics>>);

thread_local! { static METRICS: RefCell<Option<TraceMetricsContext>> = const { RefCell::new(None) }; }

pub(crate) fn capture() -> Option<TraceMetricsContext> {
    METRICS.with(|value| value.borrow().clone())
}

#[must_use]
pub(crate) struct TraceMetricsScope {
    previous: Option<TraceMetricsContext>,
    // A thread-local restoration guard must be dropped on the thread that installed it.
    _same_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

pub(crate) fn install(context: Option<TraceMetricsContext>) -> TraceMetricsScope {
    let previous = METRICS.with(|value| value.replace(context));
    TraceMetricsScope {
        previous,
        _same_thread: std::marker::PhantomData,
    }
}

impl Drop for TraceMetricsScope {
    fn drop(&mut self) {
        METRICS.with(|value| *value.borrow_mut() = self.previous.take());
    }
}

pub(crate) fn start() {
    METRICS.with(|value| {
        *value.borrow_mut() = Some(TraceMetricsContext(Arc::new(Mutex::new(
            TraceReadMetrics::default(),
        ))));
    });
}
pub(crate) fn finish() -> TraceReadMetrics {
    METRICS.with(|value| {
        value
            .borrow_mut()
            .take()
            .map_or_else(TraceReadMetrics::default, |context| {
                context
                    .0
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone()
            })
    })
}

fn update(update: impl FnOnce(&mut TraceReadMetrics)) {
    if let Some(context) = capture() {
        update(&mut context.0.lock().unwrap_or_else(|error| error.into_inner()));
    }
}

pub(crate) fn trace(bytes: usize) {
    update(|metrics| {
        metrics.trace_loads += 1;
        metrics.trace_bytes += bytes;
    });
}
pub(crate) fn context_load() {
    update(|metrics| metrics.context_loads += 1);
}
pub(crate) fn decompressed(bytes: usize) {
    update(|metrics| {
        metrics.decompressions += 1;
        metrics.context_bytes += bytes;
    });
}
pub(crate) fn lock_wait(elapsed: std::time::Duration) {
    update(|metrics| metrics.lock_wait_ns.push(elapsed.as_nanos() as u64));
}

pub(crate) fn transaction(elapsed: std::time::Duration) {
    update(|metrics| metrics.transaction_ns.push(elapsed.as_nanos() as u64));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_metrics_are_counted_in_the_submitting_scope() {
        start();
        trace(4);
        let context = capture();
        std::thread::spawn(move || {
            let _scope = install(context);
            trace(8);
            context_load();
            decompressed(16);
            transaction(std::time::Duration::from_nanos(32));
        })
        .join()
        .unwrap();
        let metrics = finish();
        assert_eq!(metrics.trace_loads, 2);
        assert_eq!(metrics.trace_bytes, 12);
        assert_eq!(metrics.context_loads, 1);
        assert_eq!(metrics.decompressions, 1);
        assert_eq!(metrics.context_bytes, 16);
        assert_eq!(metrics.transaction_ns, vec![32]);
    }

    #[test]
    fn worker_scope_restores_its_previous_measurement_after_unwind() {
        start();
        let caller = capture();
        let worker_metrics = std::thread::spawn(move || {
            start();
            trace(1);
            let panic = std::panic::catch_unwind(|| {
                let _scope = install(caller);
                trace(2);
                panic!("controlled test panic");
            });
            assert!(panic.is_err());
            trace(4);
            finish()
        })
        .join()
        .unwrap();
        let caller_metrics = finish();
        assert_eq!(caller_metrics.trace_loads, 1);
        assert_eq!(caller_metrics.trace_bytes, 2);
        assert_eq!(worker_metrics.trace_loads, 2);
        assert_eq!(worker_metrics.trace_bytes, 5);
    }
}
