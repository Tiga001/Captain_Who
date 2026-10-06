//! A byte-accounted FIFO for synchronous notification producers.
//!
//! `send` never waits for the consumer. Producers include runtime callbacks and callers holding
//! service locks, so blocking on stdout here would deadlock or block a Tokio worker. Saturation
//! fails the whole connection, wakes admission, and reserves an independent terminal error frame.
//! Already accepted frames remain ordered and drain before that error whenever the peer reads.

use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::{
    mpsc::error::{SendError, TryRecvError},
    Notify,
};

const DATA_BYTES: usize = 4 * 1024 * 1024;
const CONTROL_RESERVE_BYTES: usize = 512 * 1024;
const MAX_FRAMES: usize = 8192;
const CONTROL_RESERVE_FRAMES: usize = 64;
const MAX_MERGED_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct OutboundLimits {
    pub(crate) data_bytes: usize,
    pub(crate) control_bytes: usize,
    pub(crate) frames: usize,
    pub(crate) control_frames: usize,
    pub(crate) merge_bytes: usize,
}

impl Default for OutboundLimits {
    fn default() -> Self {
        Self {
            data_bytes: DATA_BYTES,
            control_bytes: CONTROL_RESERVE_BYTES,
            frames: MAX_FRAMES,
            control_frames: CONTROL_RESERVE_FRAMES,
            merge_bytes: MAX_MERGED_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OutboundStats {
    pub(crate) peak_bytes: usize,
    pub(crate) peak_wire_bytes: usize,
    pub(crate) peak_oversize_bytes: usize,
    pub(crate) peak_frames: usize,
    pub(crate) merged_events: usize,
    pub(crate) accepted_events: usize,
    pub(crate) serialized_bytes: usize,
    pub(crate) serialization_ns: u128,
}

struct State {
    queue: VecDeque<OutboundFrame>,
    bytes: usize,
    wire_bytes: usize,
    data_bytes: usize,
    data_frames: usize,
    senders: usize,
    closed: bool,
    failed: bool,
    overloaded: bool,
    oversize_in_use: bool,
    stats: OutboundStats,
    failure_snapshot: Option<FailureSnapshot>,
}

/// Only closed labels and counters: no method supplied by a caller, IDs, paths or payloads.
#[derive(Debug, Clone)]
struct FailureSnapshot {
    category: &'static str,
    frame_bytes: usize,
    queue_bytes: usize,
    data_bytes: usize,
    frames: usize,
    data_frames: usize,
    frame_limit: bool,
    data_frame_limit: bool,
    byte_limit: bool,
    data_byte_limit: bool,
    oversize_busy: bool,
}

struct Shared {
    state: Mutex<State>,
    ready: Notify,
    failure: Notify,
    limits: OutboundLimits,
}

/// Public through CoreServerNotificationSender; only transport constructs production channels.
pub struct OutboundSender {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for OutboundSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutboundSender")
            .field("closed", &self.is_closed())
            .finish()
    }
}

pub(crate) struct OutboundReceiver {
    shared: Arc<Shared>,
}

#[derive(Clone)]
pub(crate) struct OutboundControl {
    shared: Arc<Shared>,
}

pub(crate) struct OutboundFrame {
    pub(crate) wire: Vec<u8>,
    delta: Option<(usize, usize)>,
    data: bool,
    // One oversized non-delta response/notification is leased through flush. Existing full
    // message/done/tool-result events and RPC responses have no common protocol size cap.
    oversize: Option<Arc<Shared>>,
    queued_at: Option<std::time::Instant>,
    category: &'static str,
}

impl Drop for OutboundFrame {
    fn drop(&mut self) {
        if let Some(shared) = &self.oversize {
            shared
                .state
                .lock()
                .expect("outbound state poisoned")
                .oversize_in_use = false;
        }
    }
}

pub(crate) fn outbound_channel() -> (OutboundSender, OutboundReceiver) {
    outbound_channel_with_limits(OutboundLimits::default())
}

pub(crate) fn outbound_channel_with_limits(
    limits: OutboundLimits,
) -> (OutboundSender, OutboundReceiver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            queue: VecDeque::new(),
            bytes: 0,
            wire_bytes: 0,
            data_bytes: 0,
            data_frames: 0,
            senders: 1,
            closed: false,
            failed: false,
            overloaded: false,
            oversize_in_use: false,
            stats: OutboundStats::default(),
            failure_snapshot: None,
        }),
        ready: Notify::new(),
        failure: Notify::new(),
        limits,
    });
    (
        OutboundSender {
            shared: Arc::clone(&shared),
        },
        OutboundReceiver { shared },
    )
}

impl Clone for OutboundSender {
    fn clone(&self) -> Self {
        self.shared
            .state
            .lock()
            .expect("outbound state poisoned")
            .senders += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for OutboundSender {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().expect("outbound state poisoned");
        state.senders -= 1;
        if state.senders == 0 {
            state.closed = true;
        }
        drop(state);
        self.shared.ready.notify_one();
    }
}

impl OutboundSender {
    pub fn send(&self, value: Value) -> Result<(), SendError<Value>> {
        if self.is_closed() {
            return Err(SendError(value));
        }
        let started = std::time::Instant::now();
        let mut bytes = serde_json::to_vec(&value).expect("JSON Value serialization cannot fail");
        bytes.push(b'\n');
        let serialization_ns = started.elapsed().as_nanos();
        let data = is_body_delta(&value);
        let category = diagnostic_category(&value, data);
        mycopilot_core::performance::record(
            "outbound.serialize",
            category,
            started.elapsed(),
            bytes.len(),
        );
        let delta = ordinary_delta_range(&value, &bytes);
        let mut frame = OutboundFrame {
            wire: bytes.into_boxed_slice().into_vec(),
            delta,
            data,
            oversize: None,
            queued_at: mycopilot_core::performance::enabled().then(std::time::Instant::now),
            category,
        };
        let size = frame.wire.len();
        let limits = self.shared.limits;
        let mut state = self.shared.state.lock().expect("outbound state poisoned");
        if state.closed {
            return Err(SendError(value));
        }
        state.stats.serialized_bytes += size;
        state.stats.serialization_ns += serialization_ns;

        // Compare every byte outside the delta string, not just a guessed run key. This preserves
        // route, run, stream, and any future stage metadata; every non-delta is a FIFO boundary.
        let spare = (limits.data_bytes + limits.control_bytes - state.bytes)
            .min(limits.data_bytes.saturating_sub(state.data_bytes));
        if let Some(previous) = state.queue.back_mut() {
            if let Some((wire_added, capacity_added)) =
                try_merge(previous, &frame, limits.merge_bytes, spare)
            {
                state.bytes += capacity_added;
                state.wire_bytes += wire_added;
                state.data_bytes += capacity_added;
                state.stats.merged_events += 1;
                state.stats.accepted_events += 1;
                state.stats.peak_bytes = state.stats.peak_bytes.max(state.bytes);
                state.stats.peak_wire_bytes = state.stats.peak_wire_bytes.max(state.wire_bytes);
                drop(state);
                mycopilot_core::performance::record(
                    "outbound.merged",
                    category,
                    std::time::Duration::ZERO,
                    size,
                );
                self.shared.ready.notify_one();
                return Ok(());
            }
        }

        let oversized_frame = !data && size > limits.data_bytes + limits.control_bytes;
        let charge = if oversized_frame {
            0
        } else {
            frame.wire.capacity()
        };
        let fits = state.queue.len() < limits.frames
            && (!data || state.data_frames < limits.frames.saturating_sub(limits.control_frames))
            && state.bytes + charge <= limits.data_bytes + limits.control_bytes
            && (!data || state.data_bytes + charge <= limits.data_bytes)
            && (!oversized_frame || !state.oversize_in_use);
        if !fits {
            state.failure_snapshot = Some(FailureSnapshot {
                category,
                frame_bytes: size,
                queue_bytes: state.bytes,
                data_bytes: state.data_bytes,
                frames: state.queue.len(),
                data_frames: state.data_frames,
                frame_limit: state.queue.len() >= limits.frames,
                data_frame_limit: data
                    && state.data_frames >= limits.frames.saturating_sub(limits.control_frames),
                byte_limit: state.bytes + charge > limits.data_bytes + limits.control_bytes,
                data_byte_limit: data && state.data_bytes + charge > limits.data_bytes,
                oversize_busy: oversized_frame && state.oversize_in_use,
            });
            state.failed = true;
            state.overloaded = true;
            state.closed = true;
            drop(state);
            self.shared.ready.notify_one();
            self.shared.failure.notify_waiters();
            return Err(SendError(value));
        }
        if oversized_frame {
            state.oversize_in_use = true;
            state.stats.peak_oversize_bytes = state.stats.peak_oversize_bytes.max(size);
            frame.oversize = Some(Arc::clone(&self.shared));
        }
        state.bytes += charge;
        state.wire_bytes += if oversized_frame { 0 } else { size };
        if data {
            state.data_bytes += charge;
            state.data_frames += 1;
        }
        state.queue.push_back(frame);
        state.stats.accepted_events += 1;
        state.stats.peak_bytes = state.stats.peak_bytes.max(state.bytes);
        state.stats.peak_frames = state.stats.peak_frames.max(state.queue.len());
        state.stats.peak_wire_bytes = state.stats.peak_wire_bytes.max(state.wire_bytes);
        drop(state);
        self.shared.ready.notify_one();
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.shared
            .state
            .lock()
            .expect("outbound state poisoned")
            .closed
    }

    pub(crate) async fn failed(&self) {
        self.control().failed().await;
    }

    pub(crate) fn control(&self) -> OutboundControl {
        OutboundControl {
            shared: Arc::clone(&self.shared),
        }
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> OutboundStats {
        self.shared
            .state
            .lock()
            .expect("outbound state poisoned")
            .stats
    }
}

impl OutboundControl {
    pub(crate) fn report_failure(&self) {
        let failure = self
            .shared
            .state
            .lock()
            .expect("outbound state poisoned")
            .failure_snapshot
            .take();
        if let Some(failure) = failure {
            tracing::warn!(target: "core_performance",
                category = failure.category, frame_bytes = failure.frame_bytes,
                queue_bytes = failure.queue_bytes, data_bytes = failure.data_bytes,
                frames = failure.frames, data_frames = failure.data_frames,
                frame_limit = failure.frame_limit, data_frame_limit = failure.data_frame_limit,
                byte_limit = failure.byte_limit, data_byte_limit = failure.data_byte_limit,
                oversize_busy = failure.oversize_busy, "outbound capacity exhausted");
        }
    }

    pub(crate) fn sample_queue(&self) {
        if !mycopilot_core::performance::enabled() {
            return;
        }
        let state = self.shared.state.lock().expect("outbound state poisoned");
        let stats = state.stats;
        let (bytes, frames, oversize_in_use) =
            (state.bytes, state.queue.len(), state.oversize_in_use);
        drop(state);
        tracing::info!(target: "core_performance", bytes, frames, oversize_in_use,
            peak_bytes = stats.peak_bytes, peak_frames = stats.peak_frames,
            peak_oversize_bytes = stats.peak_oversize_bytes,
            accepted = stats.accepted_events, merged = stats.merged_events,
            "outbound queue snapshot");
    }

    pub(crate) fn close(&self) {
        self.shared
            .state
            .lock()
            .expect("outbound state poisoned")
            .closed = true;
        self.shared.ready.notify_one();
    }

    pub(crate) fn writer_failed(&self) {
        let mut state = self.shared.state.lock().expect("outbound state poisoned");
        state.failed = true;
        state.closed = true;
        drop(state);
        self.shared.failure.notify_waiters();
        self.shared.ready.notify_one();
    }

    pub(crate) fn overloaded(&self) -> bool {
        self.shared
            .state
            .lock()
            .expect("outbound state poisoned")
            .overloaded
    }

    pub(crate) async fn failed(&self) {
        loop {
            let notified = self.shared.failure.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .shared
                .state
                .lock()
                .expect("outbound state poisoned")
                .failed
            {
                return;
            }
            notified.await;
        }
    }
}

impl OutboundReceiver {
    pub(crate) fn close(&mut self) {
        self.shared
            .state
            .lock()
            .expect("outbound state poisoned")
            .closed = true;
        self.shared.ready.notify_one();
    }

    pub(crate) fn control(&self) -> OutboundControl {
        OutboundControl {
            shared: Arc::clone(&self.shared),
        }
    }

    pub(crate) fn try_recv_frame(
        &mut self,
        max_bytes: usize,
    ) -> Result<OutboundFrame, TryRecvError> {
        let mut state = self.shared.state.lock().expect("outbound state poisoned");
        if state
            .queue
            .front()
            .is_some_and(|frame| frame.wire.len() > max_bytes)
        {
            return Err(TryRecvError::Empty);
        }
        if let Some(frame) = state.queue.pop_front() {
            let charge = if frame.oversize.is_some() {
                0
            } else {
                frame.wire.capacity()
            };
            state.bytes -= charge;
            state.wire_bytes -= if frame.oversize.is_some() {
                0
            } else {
                frame.wire.len()
            };
            if frame.data {
                state.data_bytes -= charge;
                state.data_frames -= 1;
            }
            drop(state);
            if let Some(queued_at) = frame.queued_at {
                mycopilot_core::performance::record(
                    "outbound.queue",
                    frame.category,
                    queued_at.elapsed(),
                    frame.wire.len(),
                );
            }
            return Ok(frame);
        }
        if state.closed {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    pub(crate) async fn recv_frame(&mut self) -> Option<OutboundFrame> {
        loop {
            let shared = Arc::clone(&self.shared);
            let notified = shared.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            match self.try_recv_frame(usize::MAX) {
                Ok(frame) => return Some(frame),
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => notified.await,
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn recv(&mut self) -> Option<Value> {
        self.recv_frame()
            .await
            .map(|frame| serde_json::from_slice(&frame.wire).expect("encoded JSON"))
    }

    #[cfg(test)]
    pub(crate) fn try_recv(&mut self) -> Result<Value, TryRecvError> {
        self.try_recv_frame(usize::MAX)
            .map(|frame| serde_json::from_slice(&frame.wire).expect("encoded JSON"))
    }
}

fn diagnostic_category(value: &Value, data: bool) -> &'static str {
    match value.get("method").and_then(Value::as_str) {
        Some("agent.event") if data => "agent.delta",
        Some("agent.event") => "agent.event",
        Some("agent.collaboration.observerEvent") if data => "observer.delta",
        Some("agent.collaboration.observerEvent") => "observer.event",
        Some("agent.collaboration.childEvent") if data => "child.delta",
        Some("agent.collaboration.childEvent") => "child.event",
        Some("agent.workflows.runtime.changed") => "workflow.runtime",
        None if value.get("id").is_some() => "rpc.response",
        _ => "other",
    }
}

impl Drop for OutboundReceiver {
    fn drop(&mut self) {
        // Drop frames outside the lock because oversized frame leases also acquire it.
        let frames = {
            let mut state = self.shared.state.lock().expect("outbound state poisoned");
            if !state.closed {
                state.failed = true;
            }
            state.closed = true;
            std::mem::take(&mut state.queue)
        };
        drop(frames);
        self.shared.failure.notify_waiters();
        self.shared.ready.notify_one();
    }
}

fn is_body_delta(value: &Value) -> bool {
    let event = match value.get("method").and_then(Value::as_str) {
        Some("agent.event") => value.get("params"),
        Some("agent.collaboration.observerEvent" | "agent.collaboration.childEvent") => {
            value.get("params").and_then(|params| params.get("event"))
        }
        _ => None,
    };
    matches!(
        event
            .and_then(|event| event.get("type"))
            .and_then(Value::as_str),
        Some("message_delta" | "command_output" | "tool_input_progress")
    )
}

fn ordinary_delta_range(value: &Value, wire: &[u8]) -> Option<(usize, usize)> {
    if value
        .as_object()?
        .keys()
        .any(|key| !matches!(key.as_str(), "jsonrpc" | "method" | "params"))
    {
        return None;
    }
    if value.get("jsonrpc")?.as_str()? != "2.0" || value.get("method")?.as_str()? != "agent.event" {
        return None;
    }
    let params = value.get("params")?;
    if params.get("type")?.as_str()? != "message_delta" || !params.get("runId")?.is_string() {
        return None;
    }
    params.get("delta")?.as_str()?;
    // Require the current known shape. Future extra nested fields must be reviewed before merge.
    if params
        .as_object()?
        .keys()
        .any(|key| !matches!(key.as_str(), "type" | "runId" | "streamId" | "delta"))
    {
        return None;
    }
    let marker = b"\"delta\":\"";
    let start = wire.windows(marker.len()).position(|part| part == marker)? + marker.len();
    let mut end = start;
    while end < wire.len() {
        match wire[end] {
            b'\\' => end += 2,
            b'\"' => return Some((start, end)),
            _ => end += 1,
        }
    }
    None
}

fn try_merge(
    left: &mut OutboundFrame,
    right: &OutboundFrame,
    max_bytes: usize,
    spare: usize,
) -> Option<(usize, usize)> {
    let (left_start, left_end) = left.delta?;
    let (right_start, right_end) = right.delta?;
    if left.wire[..left_start] != right.wire[..right_start]
        || left.wire[left_end..] != right.wire[right_end..]
    {
        return None;
    }
    let added = right_end - right_start;
    let old_len = left.wire.len();
    let old_capacity = left.wire.capacity();
    let new_len = old_len + added;
    if new_len > max_bytes || new_len > old_capacity + spare {
        return None;
    }
    if new_len > old_capacity {
        let capacity = old_capacity
            .saturating_mul(2)
            .max(new_len)
            .min(max_bytes)
            .min(old_capacity + spare);
        let mut grown = Vec::with_capacity(capacity);
        // Account the allocator's actual reported capacity before accepting it into the queue.
        if grown.capacity() > old_capacity + spare {
            return None;
        }
        grown.extend_from_slice(&left.wire[..left_end]);
        grown.extend_from_slice(&right.wire[right_start..right_end]);
        grown.extend_from_slice(&left.wire[left_end..]);
        left.wire = grown;
    } else {
        left.wire.resize(new_len, 0);
        left.wire.copy_within(left_end..old_len, left_end + added);
        left.wire[left_end..left_end + added].copy_from_slice(&right.wire[right_start..right_end]);
    }
    left.delta = Some((left_start, left_end + added));
    Some((added, left.wire.capacity() - old_capacity))
}

pub(crate) fn overload_frame() -> &'static [u8] {
    b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32002,\"message\":\"Outbound queue capacity exceeded; this connection is closing\",\"data\":{\"code\":\"outbound_overloaded\",\"retryable\":true}}}\n"
}

#[cfg(test)]
#[path = "outbound_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "outbound_benchmarks.rs"]
mod benchmarks;
