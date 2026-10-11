//! Finite database execution admission. A reservation is not a commit or a successful RPC.
//! Sync adapters use Rayon scoped `install`: borrowed domain inputs cannot outlive their caller,
//! and the caller waits for the actual transaction. Async jobs transfer their permit to the worker.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;
use tokio::sync::Notify;

pub(super) const WRITE_JOBS: usize = 64;
pub(super) const WRITE_BYTES: usize = 64 * 1024 * 1024;
const CONTROL_RESERVE: usize = 8;
const CONTROL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageWriteClass {
    Normal,
    Control,
}

struct Admission {
    accepting: bool,
    next: u64,
    bytes: usize,
    normal: usize,
    queue: VecDeque<u64>,
}
pub(super) struct WriteGate {
    state: Mutex<Admission>,
    ready: Condvar,
    changed: Notify,
}
pub(super) struct WritePermit {
    gate: Arc<WriteGate>,
    ticket: u64,
    bytes: usize,
    class: StorageWriteClass,
    admitted: Instant,
    _probe: Option<crate::performance::QueueEntry>,
}

impl WriteGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Admission {
                accepting: true,
                next: 0,
                bytes: 0,
                normal: 0,
                queue: VecDeque::new(),
            }),
            ready: Condvar::new(),
            changed: Notify::new(),
        })
    }
    pub fn reserve(
        self: &Arc<Self>,
        bytes: usize,
        class: StorageWriteClass,
    ) -> Result<WritePermit, String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.accepting {
            return Err("storage_executor_closed".into());
        }
        let maximum = if class == StorageWriteClass::Normal {
            WRITE_BYTES - CONTROL_BYTES
        } else {
            WRITE_BYTES
        };
        if bytes > maximum {
            return Err("storage_executor_job_too_large".into());
        }
        if state.queue.len() >= WRITE_JOBS
            || bytes > WRITE_BYTES - state.bytes
            || (class == StorageWriteClass::Normal
                && (state.normal >= WRITE_JOBS - CONTROL_RESERVE
                    || bytes > (WRITE_BYTES - CONTROL_BYTES).saturating_sub(state.bytes)))
        {
            return Err("storage_executor_overloaded".into());
        }
        let ticket = state.next;
        state.next = state
            .next
            .checked_add(1)
            .ok_or("storage_executor_ticket_exhausted")?;
        state.queue.push_back(ticket);
        state.bytes += bytes;
        if class == StorageWriteClass::Normal {
            state.normal += 1;
        }
        Ok(WritePermit {
            gate: self.clone(),
            ticket,
            bytes,
            class,
            admitted: Instant::now(),
            _probe: crate::performance::track_queue("storage.queue", "writer", bytes),
        })
    }
    pub fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .accepting = false;
        self.ready.notify_all();
        self.changed.notify_waiters();
    }
    pub async fn drained(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .queue
                .is_empty()
            {
                return;
            }
            changed.await;
        }
    }
}
impl WritePermit {
    pub fn wait(&self) {
        let mut state = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.queue.front() != Some(&self.ticket) {
            state = self
                .gate
                .ready
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        crate::performance::record(
            "storage.queue_wait",
            "writer",
            self.admitted.elapsed(),
            self.bytes,
        );
    }
    pub async fn ready(&self) {
        loop {
            let changed = self.gate.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .gate
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .queue
                .front()
                == Some(&self.ticket)
            {
                break;
            }
            changed.await;
        }
        crate::performance::record(
            "storage.queue_wait",
            "writer",
            self.admitted.elapsed(),
            self.bytes,
        );
    }
}
impl Drop for WritePermit {
    fn drop(&mut self) {
        // Retire the diagnostic before releasing the real budget so sampled peaks cannot exceed
        // the enforced capacity solely because the next caller acquired a released permit.
        drop(self._probe.take());
        let mut state = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = state.queue.iter().position(|ticket| *ticket == self.ticket) {
            state.queue.remove(index);
        }
        state.bytes -= self.bytes;
        if self.class == StorageWriteClass::Normal {
            state.normal -= 1;
        }
        drop(state);
        self.gate.ready.notify_all();
        self.gate.changed.notify_waiters();
    }
}
