//! Finite database execution admission. A reservation is not a commit or a successful RPC.
//! Sync adapters use Rayon scoped `install`: borrowed domain inputs cannot outlive their caller,
//! and the caller waits for the actual transaction. Releasing a predecessor submits a ready async
//! job directly to the worker: a blocked sync caller must never depend on its runtime polling again.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, Weak};
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
    queue: VecDeque<Entry>,
}
struct Entry {
    ticket: u64,
    // The awaiting future owns the cancellable job. A strong reference here would form
    // gate -> job -> permit -> gate and retain queued work after its future is dropped.
    pending: Option<Weak<PendingWrite>>,
}
struct WriteJob {
    permit: WritePermit,
    submit: Box<dyn FnOnce(WritePermit) + Send>,
}
struct PendingWrite {
    job: Mutex<Option<WriteJob>>,
}
pub(super) struct QueuedWrite {
    pending: Arc<PendingWrite>,
}
impl PendingWrite {
    fn start(&self) {
        // Taking the job wins against cancellation. From this point the submit callback
        // owns it, just as spawning the writer did previously, even if its awaiter disappears.
        let job = self.job.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(WriteJob { permit, submit }) = job {
            permit.record_wait();
            submit(permit);
        }
    }
}
impl Drop for QueuedWrite {
    fn drop(&mut self) {
        let job = self
            .pending
            .job
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(WriteJob { permit, submit }) = job {
            // Both destructors run outside both mutexes. Release admission first: a captured
            // value's destructor may itself enter storage and must not wait for this ticket.
            drop(permit);
            drop(submit);
        }
    }
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
        state.queue.push_back(Entry {
            ticket,
            pending: None,
        });
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
        while state.queue.front().map(|entry| entry.ticket) != Some(self.ticket) {
            state = self
                .gate
                .ready
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        drop(state);
        self.record_wait();
    }
    /// Register a nonblocking submission callback, not the database operation itself. Only
    /// the FIFO head may submit to the single writer; it must never wait on admission there.
    pub fn dispatch(self, submit: impl FnOnce(WritePermit) + Send + 'static) -> QueuedWrite {
        let gate = self.gate.clone();
        let ticket = self.ticket;
        let pending = Arc::new(PendingWrite {
            job: Mutex::new(Some(WriteJob {
                permit: self,
                submit: Box::new(submit),
            })),
        });
        let is_head = {
            let mut state = gate.state.lock().unwrap_or_else(|e| e.into_inner());
            let entry = state
                .queue
                .iter_mut()
                .find(|entry| entry.ticket == ticket)
                .expect("a live permit owns its queued ticket");
            entry.pending = Some(Arc::downgrade(&pending));
            state.queue.front().map(|entry| entry.ticket) == Some(ticket)
        };
        // The predecessor may already have retired before registration. Checking the head
        // under the same mutex as retirement covers that race without an executor wakeup.
        if is_head {
            pending.start();
        }
        QueuedWrite { pending }
    }
    fn record_wait(&self) {
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
        if let Some(index) = state
            .queue
            .iter()
            .position(|entry| entry.ticket == self.ticket)
        {
            state.queue.remove(index);
        }
        state.bytes -= self.bytes;
        if self.class == StorageWriteClass::Normal {
            state.normal -= 1;
        }
        let next = state
            .queue
            .front()
            .and_then(|entry| entry.pending.as_ref())
            .and_then(Weak::upgrade);
        drop(state);
        self.gate.ready.notify_all();
        self.gate.changed.notify_waiters();
        if let Some(next) = next {
            next.start();
        }
    }
}
