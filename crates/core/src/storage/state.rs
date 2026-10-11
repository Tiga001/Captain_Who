//! SQLite ownership and the round-three compatibility boundary.
use super::commit_notifications::CommitNotifications;
use super::execution::{StorageWriteClass, WriteGate, WritePermit};
use super::{diagnostic_storage_operation, migrations, StorageEventNotifications};
use rusqlite::{Connection, OpenFlags};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

const READ_CONNECTIONS: usize = 2;
const READ_JOBS: usize = 32;
thread_local! { static CONNECTION_DEPTH: Cell<usize> = const { Cell::new(0) }; }

fn check_not_nested() -> Result<(), String> {
    CONNECTION_DEPTH.with(|depth| {
        if depth.get() == 0 {
            Ok(())
        } else {
            Err("storage_nested_execution_forbidden".into())
        }
    })
}
struct ConnectionScope(std::marker::PhantomData<std::rc::Rc<()>>);
impl ConnectionScope {
    fn enter() -> Self {
        CONNECTION_DEPTH.with(|v| v.set(v.get() + 1));
        Self(std::marker::PhantomData)
    }
}
impl Drop for ConnectionScope {
    fn drop(&mut self) {
        CONNECTION_DEPTH.with(|v| v.set(v.get() - 1));
    }
}

struct Writer {
    quarantined: AtomicBool,
    connection: Mutex<Option<Connection>>,
    notifications: CommitNotifications,
    gate: Arc<WriteGate>,
}

pub struct StorageState {
    writer: Arc<Writer>,
    worker: Arc<rayon::ThreadPool>,
    readers: Option<Arc<ReadPool>>,
    database_path: PathBuf,
    event_notifications: StorageEventNotifications,
    // One identity per opened database, shared by every read connection and publication cursor.
    pub(super) instance_identity: Arc<()>,
}

/// Legacy synchronous connection adapter. It shares the writer admission gate; never acquire
/// this guard and then wait for a new write/read job. Migrated writes use `write_sync/async`.
pub struct StorageConnectionGuard<'a> {
    guard: Option<MutexGuard<'a, Option<Connection>>>,
    notifications: &'a CommitNotifications,
    quarantined: &'a AtomicBool,
    timing: Option<(Instant, Duration)>,
    operation: &'static str,
    _scope: ConnectionScope,
    _permit: Option<WritePermit>,
}
impl std::ops::Deref for StorageConnectionGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.guard
            .as_ref()
            .and_then(|v| v.as_ref())
            .expect("live storage connection")
    }
}
impl std::ops::DerefMut for StorageConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.guard
            .as_mut()
            .and_then(|v| v.as_mut())
            .expect("live storage connection")
    }
}
impl Drop for StorageConnectionGuard<'_> {
    fn drop(&mut self) {
        if !self.is_autocommit() {
            // An operation may fail COMMIT and leave a transaction open. It must not escape to
            // the next owner. Already committed statements remain discoverable by their journal.
            if let Err(error) = self.execute_batch("ROLLBACK") {
                eprintln!("storage boundary rollback failed: {error}");
            }
        }
        if let Err(error) = self.notifications.publish_committed(self) {
            eprintln!(
                "storage committed journal wakeup failed (durable catch-up retained): {error}"
            );
        }
        if !self.is_autocommit() {
            // A connection with an uncertain live transaction is quarantined, never lent again.
            self.quarantined.store(true, Ordering::Release);
            drop(self.guard.as_mut().expect("live guard").take());
        }
        let held = self.timing.map(|(start, waited)| (start.elapsed(), waited));
        drop(self.guard.take());
        if let Some((held, waited)) = held {
            crate::performance::record("sqlite.wait", "connection", waited, 0);
            crate::performance::record("sqlite.hold", "connection", held, 0);
            crate::performance::record("sqlite.operation_wait", self.operation, waited, 0);
            crate::performance::record("sqlite.operation_hold", self.operation, held, 0);
        }
    }
}

impl Writer {
    fn lock(
        &self,
        operation: &'static str,
        permit: Option<WritePermit>,
    ) -> Result<StorageConnectionGuard<'_>, String> {
        let start = crate::performance::enabled().then(Instant::now);
        let guard = self
            .connection
            .lock()
            .map_err(|_| "数据库连接状态不可用。".to_string())?;
        if self.quarantined.load(Ordering::Acquire) {
            return Err("storage_writer_quarantined".into());
        }
        if guard.is_none() {
            return Err("storage_executor_closed".into());
        }
        Ok(StorageConnectionGuard {
            guard: Some(guard),
            notifications: &self.notifications,
            quarantined: &self.quarantined,
            timing: start.map(|v| (Instant::now(), v.elapsed())),
            operation,
            _scope: ConnectionScope::enter(),
            _permit: permit,
        })
    }
}

impl StorageState {
    pub fn open(database_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = database_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(database_path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        migrations::run_migrations(&connection)?;
        let memory = database_path == Path::new(":memory:");
        if !memory {
            // Bundled 3.53.2 includes the WAL reset fix (3.51.3). Do not enable multi-connection
            // WAL with a build silently linked to an older system library.
            if rusqlite::version_number() < 3_051_003 {
                return Err("SQLite WAL concurrency fix is required".into());
            }
            let mode: String =
                connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
            if mode != "wal" {
                return Err(format!("WAL unavailable: {mode}").into());
            }
        }
        connection.execute_batch("PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=1000;")?;
        let event_notifications = StorageEventNotifications::default();
        let notifications = CommitNotifications::install(&connection, event_notifications.clone())?;
        let readers = if memory {
            None
        } else {
            Some(ReadPool::open(database_path)?)
        };
        Ok(Self {
            writer: Arc::new(Writer {
                quarantined: AtomicBool::new(false),
                connection: Mutex::new(Some(connection)),
                notifications,
                gate: WriteGate::new(),
            }),
            worker: Arc::new(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .thread_name(|_| "core-sqlite-writer".into())
                    .build()?,
            ),
            readers,
            database_path: database_path.to_path_buf(),
            event_notifications,
            instance_identity: Arc::new(()),
        })
    }

    #[track_caller]
    pub fn connection(&self) -> Result<StorageConnectionGuard<'_>, String> {
        check_not_nested()?;
        let permit = self
            .writer
            .gate
            .reserve(16 * 1024, StorageWriteClass::Normal)?;
        permit.wait();
        self.writer.lock(
            diagnostic_storage_operation(std::panic::Location::caller().file()),
            Some(permit),
        )
    }

    /// The caller blocks until the actual transaction finishes. No success is reported on enqueue.
    #[track_caller]
    pub fn write_sync<R: Send>(
        &self,
        estimated_bytes: usize,
        class: StorageWriteClass,
        work: impl FnOnce(&mut Connection) -> R + Send,
    ) -> Result<R, String> {
        check_not_nested()?;
        let permit = self.writer.gate.reserve(estimated_bytes, class)?;
        permit.wait(); // FIFO admission happens before entering the single worker, never inside it.
        let operation = diagnostic_storage_operation(std::panic::Location::caller().file());
        #[cfg(test)]
        let metrics = super::trace_performance_metrics::capture();
        let scheduled = crate::performance::enabled().then(Instant::now);
        self.worker.install(|| {
            if let Some(scheduled) = scheduled {
                crate::performance::record("storage.worker_wait", "writer", scheduled.elapsed(), 0);
            }
            #[cfg(test)]
            let _metrics = super::trace_performance_metrics::install(metrics);
            let mut connection = self.writer.lock(operation, Some(permit))?;
            let result = work(&mut connection);
            if !connection.is_autocommit() {
                return Err("storage_transaction_left_open".into());
            }
            Ok(result)
        })
    }

    /// Queued cancellation drops its ticket. Once started, the job owns its data and admission
    /// until completion even if its awaiter disappears; shutdown drains those mutations.
    #[track_caller]
    pub fn write_async<R: Send + 'static>(
        &self,
        estimated_bytes: usize,
        class: StorageWriteClass,
        work: impl FnOnce(&mut Connection) -> R + Send + 'static,
    ) -> impl std::future::Future<Output = Result<R, String>> + Send + 'static {
        let context = check_not_nested();
        let writer = self.writer.clone();
        let worker = self.worker.clone();
        let operation = diagnostic_storage_operation(std::panic::Location::caller().file());
        #[cfg(test)]
        let metrics = super::trace_performance_metrics::capture();
        async move {
            context?;
            check_not_nested()?;
            // Constructing but never polling a future is not submission and cannot pin drain.
            let permit = writer.gate.reserve(estimated_bytes, class)?;
            permit.ready().await;
            let (send, receive) = tokio::sync::oneshot::channel();
            let scheduled = crate::performance::enabled().then(Instant::now);
            worker.spawn(move || {
                if let Some(scheduled) = scheduled {
                    crate::performance::record(
                        "storage.worker_wait",
                        "writer",
                        scheduled.elapsed(),
                        0,
                    );
                }
                #[cfg(test)]
                let _metrics = super::trace_performance_metrics::install(metrics);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut connection = writer.lock(operation, Some(permit))?;
                    let value = work(&mut connection);
                    if !connection.is_autocommit() {
                        return Err("storage_transaction_left_open".into());
                    }
                    Ok(value)
                }))
                .unwrap_or_else(|_| Err("storage_worker_panicked".into()));
                let _ = send.send(result);
            });
            receive
                .await
                .map_err(|_| "storage_worker_unavailable".to_string())?
        }
    }

    #[track_caller]
    pub fn read_connection(&self) -> Result<StorageReadGuard<'_>, String> {
        check_not_nested()?;
        match &self.readers {
            Some(pool) => Ok(StorageReadGuard::Pooled(pool.acquire()?)),
            None => Ok(StorageReadGuard::Memory(self.connection()?)),
        }
    }

    pub fn event_notifications(&self) -> StorageEventNotifications {
        self.event_notifications.clone()
    }

    #[cfg(test)]
    pub(crate) fn configure_read_connections_for_test(
        &self,
        mut configure: impl FnMut(&mut Connection),
    ) {
        check_not_nested().unwrap();
        if let Some(readers) = &self.readers {
            let mut state = readers.state.lock().unwrap();
            assert_eq!(state.jobs, 0, "configure only idle test read connections");
            for connection in &mut state.connections {
                configure(connection);
            }
        } else {
            configure(&mut self.connection().unwrap());
        }
    }

    pub fn checkpoint(
        &self,
    ) -> impl std::future::Future<Output = Result<WalCheckpoint, String>> + Send + 'static {
        let path = self.database_path.clone();
        let result = self.write_async(1024, StorageWriteClass::Control, move |connection| {
            checkpoint(connection, &path, false)
        });
        async move { result.await? }
    }

    pub async fn shutdown(&self) -> Result<WalCheckpoint, String> {
        check_not_nested()?;
        self.writer.gate.close();
        if let Some(readers) = &self.readers {
            readers.close();
        }
        self.writer.gate.drained().await;
        if let Some(readers) = &self.readers {
            readers.drained().await;
        }
        let writer = self.writer.clone();
        let readers = self.readers.clone();
        let path = self.database_path.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        self.worker.spawn(move || {
            if let Some(readers) = readers {
                readers
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .connections
                    .clear();
            }
            let result = (|| {
                let mut guard = writer
                    .connection
                    .lock()
                    .map_err(|_| "storage_connection_poisoned".to_string())?;
                if writer.quarantined.load(Ordering::Acquire) {
                    return Err("storage_writer_quarantined".into());
                }
                let Some(connection) = guard.as_ref() else {
                    return Ok(WalCheckpoint::default());
                };
                writer
                    .notifications
                    .publish_committed(connection)
                    .map_err(|e| e.to_string())?;
                let progress = checkpoint(connection, &path, true)?;
                let connection = guard.take().expect("checked open");
                if let Err((connection, error)) = connection.close() {
                    *guard = Some(connection);
                    return Err(error.to_string());
                }
                Ok(progress)
            })();
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| "storage_close_worker_unavailable".to_string())?
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WalCheckpoint {
    pub busy: i64,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
    pub wal_bytes: u64,
}
fn checkpoint(
    connection: &Connection,
    path: &Path,
    truncate: bool,
) -> Result<WalCheckpoint, String> {
    let start = Instant::now();
    let mut progress = connection
        .query_row(
            if truncate {
                "PRAGMA wal_checkpoint(TRUNCATE)"
            } else {
                "PRAGMA wal_checkpoint(PASSIVE)"
            },
            [],
            |row| {
                Ok(WalCheckpoint {
                    busy: row.get(0)?,
                    log_frames: row.get(1)?,
                    checkpointed_frames: row.get(2)?,
                    wal_bytes: 0,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    progress.wal_bytes = std::fs::metadata(Path::new(&wal))
        .map(|v| v.len())
        .unwrap_or(0);
    crate::performance::record(
        "sqlite.checkpoint",
        if truncate { "truncate" } else { "passive" },
        start.elapsed(),
        progress.wal_bytes as usize,
    );
    for (operation, count) in [
        ("log", progress.log_frames),
        ("checkpointed", progress.checkpointed_frames),
        ("busy", progress.busy),
    ] {
        crate::performance::record(
            "sqlite.wal_frames",
            operation,
            Duration::ZERO,
            count.max(0) as usize,
        );
    }
    Ok(progress)
}

struct ReadState {
    accepting: bool,
    failed: bool,
    jobs: usize,
    connections: Vec<Connection>,
}
struct ReadPool {
    state: Mutex<ReadState>,
    available: Condvar,
    changed: Notify,
}
impl ReadPool {
    fn open(path: &Path) -> Result<Arc<Self>, rusqlite::Error> {
        let mut connections = Vec::new();
        for _ in 0..READ_CONNECTIONS {
            let connection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            connection.busy_timeout(Duration::from_secs(5))?;
            connection.execute_batch("PRAGMA query_only=ON;")?;
            connections.push(connection);
        }
        Ok(Arc::new(Self {
            state: Mutex::new(ReadState {
                accepting: true,
                failed: false,
                jobs: 0,
                connections,
            }),
            available: Condvar::new(),
            changed: Notify::new(),
        }))
    }
    fn acquire(self: &Arc<Self>) -> Result<PooledReadGuard, String> {
        let start = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.accepting {
            return Err("storage_executor_closed".into());
        }
        if state.jobs >= READ_JOBS {
            return Err("storage_read_pool_overloaded".into());
        }
        state.jobs += 1;
        let probe = crate::performance::track_queue("storage.queue", "reader", 1024);
        while state.connections.is_empty() {
            if state.failed {
                state.jobs -= 1;
                drop(state);
                self.changed.notify_waiters();
                return Err("storage_read_pool_quarantined".into());
            }
            state = self
                .available
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        let connection = state.connections.pop();
        crate::performance::record("sqlite.read_wait", "reader", start.elapsed(), 0);
        Ok(PooledReadGuard {
            connection,
            pool: self.clone(),
            started: Instant::now(),
            _scope: ConnectionScope::enter(),
            _probe: probe,
        })
    }
    fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .accepting = false;
    }
    async fn drained(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state.lock().unwrap_or_else(|e| e.into_inner()).jobs == 0 {
                return;
            }
            changed.await;
        }
    }
}
pub struct PooledReadGuard {
    connection: Option<Connection>,
    pool: Arc<ReadPool>,
    started: Instant,
    _scope: ConnectionScope,
    _probe: Option<crate::performance::QueueEntry>,
}
impl Drop for PooledReadGuard {
    fn drop(&mut self) {
        let connection = self.connection.take().expect("live reader");
        if !connection.is_autocommit() {
            if let Err(error) = connection.execute_batch("ROLLBACK") {
                eprintln!("storage read snapshot rollback failed: {error}");
            }
        }
        crate::performance::record("sqlite.read_hold", "reader", self.started.elapsed(), 0);
        let mut state = self.pool.state.lock().unwrap_or_else(|e| e.into_inner());
        drop(self._probe.take());
        if connection.is_autocommit() {
            state.connections.push(connection);
        } else {
            state.failed = true;
            state.accepting = false;
            drop(connection);
        }
        state.jobs -= 1;
        drop(state);
        self.pool.available.notify_all();
        self.pool.changed.notify_waiters();
    }
}
pub enum StorageReadGuard<'a> {
    Pooled(PooledReadGuard),
    Memory(StorageConnectionGuard<'a>),
}
impl std::ops::Deref for StorageReadGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        match self {
            Self::Pooled(v) => v.connection.as_ref().expect("live reader"),
            Self::Memory(v) => v,
        }
    }
}
impl std::ops::DerefMut for StorageReadGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        match self {
            Self::Pooled(v) => v.connection.as_mut().expect("live reader"),
            Self::Memory(v) => v,
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod review_tests;
