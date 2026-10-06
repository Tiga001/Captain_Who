//! Coalesce committed organization changes before reading their full runtime projection.
//! The queue is a process-local hint; SQLite and explicit runtime reads remain authoritative.
use super::{AgentService, CoreServerNotificationSender};
use mycopilot_core::workflow_execution::{PreferenceUpdate, RuntimeSnapshot};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

const REFRESH_WINDOW: Duration = Duration::from_millis(100);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_PREFERENCE_NODES: usize = 128;

type PreferenceKey = (String, String, bool); // node, original conversation, permission field

#[derive(Debug)]
enum ProjectionFailure {
    InstanceMissing,
    Temporary(String),
}

type Project = Arc<dyn Fn(&str) -> Result<RuntimeSnapshot, ProjectionFailure> + Send + Sync>;

fn classify_storage_projection_error(error: String) -> ProjectionFailure {
    // The existing storage API exposes String errors. Match only its authoritative failed
    // existence check; SQLite, decoding and other errors must retain pending fresh edits.
    if error == "Organization instance no longer exists" {
        ProjectionFailure::InstanceMissing
    } else {
        ProjectionFailure::Temporary(error)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Dormant,
    Active,
    Draining,
    Closed,
}

struct Pending {
    due: Instant,
    failures: u32,
    preferences: BTreeMap<PreferenceKey, PreferenceUpdate>,
}

impl Pending {
    fn new(due: Instant) -> Self {
        Self {
            due,
            failures: 0,
            preferences: BTreeMap::new(),
        }
    }

    fn merge(&mut self, updates: Vec<PreferenceUpdate>) {
        for update in updates {
            for permission in [false, true] {
                if if permission {
                    update.permission_mode.is_none()
                } else {
                    update.model_id.is_none()
                } {
                    continue;
                }
                let key = (
                    update.node_id.clone(),
                    update.conversation_id.clone(),
                    permission,
                );
                if self
                    .preferences
                    .get(&key)
                    .is_some_and(|old| old.organization_revision >= update.organization_revision)
                {
                    continue;
                }
                let mut field = update.clone();
                if permission {
                    field.model_id = None;
                } else {
                    field.permission_mode = None;
                }
                self.preferences.insert(key, field);
            }
        }
    }

    /// Keep each field's own revision. Rebinding one node during a window retains edits to both
    /// original conversations; those rare conflicting identities are sent in separate snapshots.
    fn preference_batches(&self) -> Vec<Vec<PreferenceUpdate>> {
        let mut fields = self.preferences.values().cloned().collect::<Vec<_>>();
        fields.sort_by(|a, b| {
            (a.organization_revision, &a.node_id, &a.conversation_id).cmp(&(
                b.organization_revision,
                &b.node_id,
                &b.conversation_id,
            ))
        });
        let mut batches = vec![Vec::<PreferenceUpdate>::new()];
        let mut nodes = HashMap::<String, String>::new();
        for field in fields {
            let conflict = nodes
                .get(&field.node_id)
                .is_some_and(|conversation| conversation != &field.conversation_id);
            if conflict
                || (nodes.len() == MAX_PREFERENCE_NODES && !nodes.contains_key(&field.node_id))
            {
                batches.push(Vec::new());
                nodes.clear();
            }
            nodes.insert(field.node_id.clone(), field.conversation_id.clone());
            let batch = batches.last_mut().unwrap();
            if let Some(existing) = batch.iter_mut().find(|update| {
                update.node_id == field.node_id
                    && update.organization_revision == field.organization_revision
            }) {
                if field.model_id.is_some() {
                    existing.model_id = field.model_id;
                }
                if field.permission_mode.is_some() {
                    existing.permission_mode = field.permission_mode;
                }
            } else {
                batch.push(field);
            }
        }
        batches
    }
}

#[derive(Default)]
struct State {
    phase: Phase,
    pending: BTreeMap<String, Pending>,
}

#[derive(Clone, Default)]
pub(super) struct WorkflowRuntimePublications {
    state: Arc<Mutex<State>>,
    wake: Arc<Notify>,
    publication_gate: Arc<Mutex<bool>>,
}

enum Next {
    Finished,
    Wait(Option<Instant>),
    Publish(String, Pending),
}

impl WorkflowRuntimePublications {
    fn start(&self) -> Result<(), String> {
        let mut open = self
            .publication_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.phase != Phase::Dormant {
            return Err("Organization runtime publisher has already started or closed.".into());
        }
        state.phase = Phase::Active;
        *open = true;
        Ok(())
    }

    /// Only unstarted, synchronous service users take the inline path. Closed workers never do.
    pub(super) fn enqueue(
        &self,
        instance: &str,
        updates: Vec<PreferenceUpdate>,
    ) -> Option<Vec<PreferenceUpdate>> {
        self.enqueue_at(instance, updates, Instant::now())
    }

    fn enqueue_at(
        &self,
        instance: &str,
        updates: Vec<PreferenceUpdate>,
        now: Instant,
    ) -> Option<Vec<PreferenceUpdate>> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match state.phase {
            Phase::Dormant => return Some(updates),
            Phase::Closed | Phase::Draining => return None,
            Phase::Active => {}
        }
        // The first change fixes this deadline. A continuously active organization cannot keep
        // postponing its refresh as it would with a trailing debounce.
        state
            .pending
            .entry(instance.into())
            .or_insert_with(|| Pending::new(now + REFRESH_WINDOW))
            .merge(updates);
        drop(state);
        self.wake.notify_one();
        None
    }

    fn next(&self, now: Instant) -> Next {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.phase == Phase::Closed {
            return Next::Finished;
        }
        let earliest = state
            .pending
            .iter()
            .min_by_key(|(_, pending)| pending.due)
            .map(|(instance, pending)| (instance.clone(), pending.due));
        match earliest {
            Some((instance, due)) if due <= now || state.phase == Phase::Draining => {
                Next::Publish(instance.clone(), state.pending.remove(&instance).unwrap())
            }
            Some((_, due)) => Next::Wait(Some(due)),
            None if state.phase == Phase::Draining => {
                state.phase = Phase::Closed;
                Next::Finished
            }
            None => Next::Wait(None),
        }
    }

    fn retry(&self, instance: String, failed: Pending, now: Instant) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        // Shutdown makes at most one final attempt. Never delay termination on retry sleeps.
        if state.phase != Phase::Active {
            return;
        }
        let failures = failed.failures.saturating_add(1);
        let delay = Duration::from_millis(100 * (1 << failures.min(6))).min(MAX_RETRY_DELAY);
        let retry_at = now + delay;
        let pending = state
            .pending
            .entry(instance)
            .or_insert_with(|| Pending::new(retry_at));
        // New mutations must not bypass the cooldown, and newer field revisions must win over
        // an older projection which failed while those mutations were being committed.
        pending.due = pending.due.max(retry_at);
        pending.failures = failures;
        pending.merge(failed.preferences.into_values().collect());
        drop(state);
        self.wake.notify_one();
    }

    fn draining(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.phase == Phase::Active {
            state.phase = Phase::Draining;
        }
        drop(state);
        self.wake.notify_one();
    }

    fn close(&self) {
        // Serialize the close cut with enqueueing a completed notification. This separate gate
        // never blocks producers marking dirty, and is never held during SQLite reads.
        let mut open = self
            .publication_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *open = false;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.phase = Phase::Closed;
        state.pending.clear();
        drop(state);
        self.wake.notify_one();
    }

    fn send_if_open(
        &self,
        sender: &CoreServerNotificationSender,
        value: serde_json::Value,
    ) -> Result<(), ()> {
        let open = self
            .publication_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !*open {
            return Ok(());
        }
        sender.send(value).map_err(|_| ())
    }

    fn is_closed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .phase
            == Phase::Closed
    }
}

pub(crate) struct WorkflowRuntimePublisher {
    queue: WorkflowRuntimePublications,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl AgentService {
    /// Bootstrap owns this worker explicitly; constructing a service never starts a runtime or
    /// thread. Start before recovered deliveries, and drain after all business producers settle.
    pub(crate) fn start_workflow_runtime_publisher(
        &self,
        notifications: CoreServerNotificationSender,
    ) -> Result<WorkflowRuntimePublisher, String> {
        self.workflow_runtime_publications.start()?;
        let storage = Arc::clone(&self.storage);
        let project = Arc::new(move |instance: &str| {
            storage
                .workflow_execution_runtime(instance)
                .map_err(classify_storage_projection_error)
        });
        Ok(spawn_worker(
            self.workflow_runtime_publications.clone(),
            notifications,
            project,
        ))
    }
}

impl WorkflowRuntimePublisher {
    pub(crate) async fn shutdown(mut self, timeout: Duration) -> Result<(), String> {
        self.queue.draining();
        let mut task = self.task.take().expect("publisher owns its worker");
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(result) => {
                result.map_err(|error| format!("Organization runtime publisher failed: {error}"))
            }
            Err(_) => {
                // Blocking SQLite reads cannot be aborted. Fence further publication and let an
                // in-flight read finish naturally; no new task/read will be admitted afterwards.
                self.queue.close();
                Err("Organization runtime publisher drain timed out; durable runtime reads recover current state.".into())
            }
        }
    }
}

impl Drop for WorkflowRuntimePublisher {
    fn drop(&mut self) {
        // Early bootstrap failures close this owner too, without leaking a sleeping worker.
        self.queue.close();
    }
}

fn spawn_worker(
    queue: WorkflowRuntimePublications,
    notifications: CoreServerNotificationSender,
    project: Project,
) -> WorkflowRuntimePublisher {
    let worker_queue = queue.clone();
    let task = tokio::spawn(async move {
        loop {
            if notifications.is_closed() {
                worker_queue.close();
            }
            match worker_queue.next(Instant::now()) {
                Next::Finished => break,
                Next::Wait(deadline) => match deadline {
                    Some(deadline) => tokio::select! {
                        _ = worker_queue.wake.notified() => {},
                        _ = tokio::time::sleep_until(deadline) => {},
                    },
                    None => worker_queue.wake.notified().await,
                },
                Next::Publish(instance, pending) => {
                    let project = project.clone();
                    let project_instance = instance.clone();
                    let publications = pending.preference_batches();
                    let sender = notifications.clone();
                    let publication_queue = worker_queue.clone();
                    // Only one projection is admitted at a time. Neither queue nor business locks
                    // are held across SQLite work, JSON encoding or outbound serialization.
                    let result = tokio::task::spawn_blocking(move || {
                        publish_snapshot(
                            &publication_queue,
                            &sender,
                            &project,
                            &project_instance,
                            publications,
                        )
                    })
                    .await;
                    let failure = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(error),
                        Err(error) => Some(format!("projection worker failed: {error}")),
                    };
                    if let Some(error) = failure {
                        let attempt = pending.failures.saturating_add(1);
                        if attempt.is_power_of_two() {
                            eprintln!("organization runtime projection failed (attempt {attempt}); pending updates retained until recovery or shutdown: {error}");
                        }
                        worker_queue.retry(instance, pending, Instant::now());
                    }
                }
            }
        }
    });
    WorkflowRuntimePublisher {
        queue,
        task: Some(task),
    }
}

fn publish_snapshot(
    queue: &WorkflowRuntimePublications,
    sender: &CoreServerNotificationSender,
    project: &Project,
    instance: &str,
    publications: Vec<Vec<PreferenceUpdate>>,
) -> Result<(), String> {
    if queue.is_closed() || sender.is_closed() {
        return Ok(());
    }
    let snapshot = {
        let _timing = mycopilot_core::performance::Span::new("workflow.runtime", "projection");
        project(instance)
    };
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        // No retry can restore a deleted organization. A newer queued dirty is left intact so
        // a concurrently recreated instance is checked once rather than accidentally erased.
        Err(ProjectionFailure::InstanceMissing) => return Ok(()),
        Err(ProjectionFailure::Temporary(error)) => return Err(error),
    };
    let mut notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "agent.workflows.runtime.changed",
    });
    notification["params"] = serde_json::to_value(snapshot).map_err(|error| error.to_string())?;
    let mut publications = publications.into_iter().peekable();
    while let Some(preferences) = publications.next() {
        if queue.is_closed() || sender.is_closed() {
            return Ok(());
        }
        let params = &mut notification["params"];
        if preferences.is_empty() {
            params.as_object_mut().unwrap().remove("preferenceUpdates");
        } else {
            params["preferenceUpdates"] =
                serde_json::to_value(preferences).map_err(|error| error.to_string())?;
        }
        // The ordinary single notification moves directly into transport. Clone the projection
        // only for earlier batches when a window contains conflicting historical bindings.
        let outgoing = if publications.peek().is_none() {
            std::mem::take(&mut notification)
        } else {
            notification.clone()
        };
        if queue.send_if_open(sender, outgoing).is_err() {
            queue.close();
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
