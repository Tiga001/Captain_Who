use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::{watch, OwnedSemaphorePermit};
use tokio::task::{JoinHandle, JoinSet};

// Admission includes running work and queued work, including jobs waiting on an ordering fence.
const READ_CONCURRENCY: usize = 4;
const READ_CAPACITY: usize = 64;
const WRITE_CAPACITY: usize = 64;
const CONTROL_CAPACITY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RpcDispatchClass {
    Read,
    Write,
    Control,
}

/// Unknown operations conservatively remain writes. Never infer read-only from a method prefix:
/// workflow mutations and attachment imports share namespaces with actual reads.
pub(crate) fn rpc_dispatch_class(request: &JsonRpcRequest) -> RpcDispatchClass {
    use mycopilot_protocol_rs::*;
    if request.method == "agent.workflows.request" {
        return match request
            .params
            .as_ref()
            .and_then(|p| p.get("operation"))
            .and_then(Value::as_str)
        {
            Some("list" | "listInstances" | "validate" | "runtimeSnapshot") => {
                RpcDispatchClass::Read
            }
            _ => RpcDispatchClass::Write,
        };
    }
    if matches!(
        request.method.as_str(),
        AGENT_CANCEL_RUN_METHOD
            | AGENT_CANCEL_ACTION_METHOD
            | AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD
    ) {
        return RpcDispatchClass::Control;
    }
    if is_blocking_read_method(&request.method)
        || matches!(
            request.method.as_str(),
            HUMAN_INTERACTION_GET_SETTINGS_METHOD
                | HUMAN_INTERACTION_LIST_REQUESTS_METHOD
                | AUTOMATION_LIST_METHOD
                | AUTOMATION_GET_METHOD
                | AUTOMATION_RUNS_LIST_METHOD
                | AUTOMATION_ATTENTION_SUMMARY_METHOD
                | NOTIFICATION_LIST_METHOD
                | NOTIFICATION_SUMMARY_METHOD
                | NOTIFICATION_SETTINGS_GET_METHOD
                | OFFICE_GET_STATUS_METHOD
                | STORAGE_LOAD_BROWSER_PREFERENCES_METHOD
                | STORAGE_LIST_BROWSER_HISTORY_METHOD
                | STORAGE_SUMMARIZE_BROWSER_OWNED_DATA_METHOD
                | AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD
                | AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD
                | AGENT_GET_MANUAL_CONTEXT_COMPACTION_STATUS_METHOD
                | "storage.loadRunWorkspace"
                | "storage.resolveRunWorkspacePath"
                | "storage.resolveRunAttachmentFile"
        )
    {
        RpcDispatchClass::Read
    } else {
        RpcDispatchClass::Write
    }
}

/// These are sequencing hints, not authority. Handlers still resolve/validate every owner. Run
/// cancellation cannot refer to a not-yet-admitted new turn: its runId is issued by the handler.
pub(crate) fn rpc_ordering_key(request: &JsonRpcRequest) -> Option<String> {
    let params = request.params.as_ref()?;
    if request.method != STORAGE_REGISTER_BROWSER_DOWNLOAD_METHOD {
        if let Some(run) = params.get("runId").and_then(Value::as_str) {
            return Some(format!("run:{run}"));
        }
    }
    let conversation = match request.method.as_str() {
        AGENT_REWRITE_CONVERSATION_TURN_METHOD => params.get("turn")?.get("conversationId"),
        AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD => params.get("rootConversationId"),
        mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD
        | mycopilot_protocol_rs::AUTOMATION_UPDATE_METHOD => {
            params.get("destination")?.get("conversationId")
        }
        STORAGE_SAVE_CONVERSATION_META_METHOD => params.get("id"),
        STORAGE_SAVE_COMPOSER_DRAFT_METHOD => params.get("draft")?.get("scopeId"),
        STORAGE_SAVE_COMPOSER_DRAFT_MESSAGE_METHOD => params.get("scopeId"),
        _ => params.get("conversationId"),
    };
    conversation
        .and_then(Value::as_str)
        .map(|conversation| format!("conversation:{conversation}"))
        .or_else(|| {
            params
                .get("runId")
                .and_then(Value::as_str)
                .map(|run| format!("run:{run}"))
        })
}

type Completion = watch::Receiver<bool>;
pub(crate) type RpcOwnerResolver = Box<dyn FnOnce() -> Option<String> + Send>;
struct OwnerResolution {
    resolve: RpcOwnerResolver,
    // Admission-time snapshot: later writes must never become dependencies of an earlier cancel.
    candidates: HashMap<String, Completion>,
}

#[derive(Default)]
struct MutationOrder {
    last_write: Option<Completion>,
    controls: Vec<Completion>,
    owners: HashMap<String, Completion>,
}

struct RpcJob {
    request_id: JsonRpcId,
    task: Box<dyn FnOnce() -> Value + Send>,
    admitted_at: std::time::Instant,
    dependencies: Vec<Completion>,
    owner_resolution: Option<OwnerResolution>,
    completion: Option<watch::Sender<bool>>,
    _permit: OwnedSemaphorePermit,
}

struct Lane {
    admission: Arc<Semaphore>,
    sender: mpsc::Sender<RpcJob>,
    manager: JoinHandle<()>,
}

impl Lane {
    fn new(
        class: RpcDispatchClass,
        capacity: usize,
        concurrency: usize,
        stopping: Arc<AtomicBool>,
        outbound: crate::transport::OutboundSender,
    ) -> Self {
        let admission = Arc::new(Semaphore::new(capacity));
        let (sender, mut receiver) = mpsc::channel::<RpcJob>(capacity);
        let execution = Arc::new(Semaphore::new(concurrency));
        let task_capacity = if class == RpcDispatchClass::Control {
            capacity
        } else {
            concurrency
        };
        let manager = tokio::spawn(async move {
            let mut running = JoinSet::new();
            loop {
                // Waiting controls do not occupy execution slots: an owner fence must not
                // prevent another owner's cancellation from being received and executed.
                // Their futures and execution-permit waiters are still bounded by admission.
                if running.len() >= task_capacity {
                    let _ = running.join_next().await;
                    continue;
                }
                tokio::select! {
                    biased;
                    Some(_) = running.join_next(), if !running.is_empty() => {}
                    job = receiver.recv() => match job {
                        Some(job) => {
                            let stopping = Arc::clone(&stopping);
                            let outbound = outbound.clone();
                            running.spawn(execute_job(class, job, stopping, outbound, Arc::clone(&execution)));
                        }
                        None => break,
                    }
                }
            }
            while running.join_next().await.is_some() {}
        });
        Self {
            admission,
            sender,
            manager,
        }
    }

    async fn shutdown(self) -> io::Result<()> {
        self.admission.close();
        drop(self.sender);
        self.manager
            .await
            .map_err(|error| io::Error::other(format!("RPC dispatcher stopped: {error}")))
    }
}

async fn execute_job(
    class: RpcDispatchClass,
    mut job: RpcJob,
    stopping: Arc<AtomicBool>,
    outbound: crate::transport::OutboundSender,
    execution: Arc<Semaphore>,
) {
    let mut resolution_error = None;
    let resolution_started = std::time::Instant::now();
    if !stopping.load(Ordering::Acquire) {
        if let Some(resolution) = job.owner_resolution.take() {
            // Resolving a run's conversation can touch the existing in-memory/DB locks. It
            // therefore runs off stdin and under the same bounded blocking concurrency.
            let _execution = execution
                .clone()
                .acquire_owned()
                .await
                .expect("lane execution remains open");
            if !stopping.load(Ordering::Acquire) {
                let resolving_stop = Arc::clone(&stopping);
                match tokio::task::spawn_blocking(move || {
                    if resolving_stop.load(Ordering::Acquire) {
                        None
                    } else {
                        (resolution.resolve)()
                    }
                })
                .await
                {
                    Ok(Some(owner)) => {
                        if let Some(dependency) = resolution.candidates.get(&owner) {
                            job.dependencies.push(dependency.clone());
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        resolution_error = Some(response_error(
                            Some(job.request_id.clone()),
                            -32603,
                            format!("RPC owner resolution failed: {error}"),
                        ))
                    }
                }
            }
            // Release before waiting on owner work so an unrelated ready control can execute.
        }
    }
    let owner_resolution_us = resolution_started.elapsed().as_micros() as u64;
    for dependency in &mut job.dependencies {
        while !*dependency.borrow_and_update() {
            if dependency.changed().await.is_err() {
                break;
            }
        }
    }
    let _execution = execution
        .acquire_owned()
        .await
        .expect("lane execution remains open");
    let started_at = std::time::Instant::now();
    let response = if stopping.load(Ordering::Acquire) {
        unavailable_response(job.request_id.clone(), false)
    } else if let Some(error) = resolution_error {
        error
    } else {
        // Never abort this join on shutdown: a blocking mutation may already have committed.
        let running_stop = Arc::clone(&stopping);
        let running_id = job.request_id.clone();
        match tokio::task::spawn_blocking(move || {
            // A submitted blocking task can itself queue behind unrelated pool work. Only
            // crossing this boundary starts the handler; shutdown still cancels pool waiters.
            if running_stop.load(Ordering::Acquire) {
                unavailable_response(running_id, false)
            } else {
                (job.task)()
            }
        })
        .await
        {
            Ok(response) => response,
            Err(error) => response_error(
                Some(job.request_id.clone()),
                -32603,
                format!("RPC worker failed: {error}"),
            ),
        }
    };
    let completed_at = std::time::Instant::now();
    let lane = match class {
        RpcDispatchClass::Read => "read",
        RpcDispatchClass::Write => "write",
        RpcDispatchClass::Control => "control",
    };
    mycopilot_core::performance::record(
        "rpc.queue",
        lane,
        started_at.duration_since(job.admitted_at),
        0,
    );
    mycopilot_core::performance::record(
        "rpc.owner_resolution",
        lane,
        Duration::from_micros(owner_resolution_us),
        0,
    );
    mycopilot_core::performance::record(
        "rpc.handler",
        lane,
        completed_at.duration_since(started_at),
        0,
    );
    let _ = enqueue_outbound(&outbound, response);
    if let Some(completion) = job.completion {
        let _ = completion.send(true);
    }
}

/// Owned by one request loop. Every accepted request has a bounded queue slot and exactly one
/// response attempt before shutdown returns. EOF drains accepted work; explicit shutdown rejects
/// queued work and joins running handlers. It never advertises cancellation as rollback.
pub(crate) struct RpcRequestDispatcher {
    read: Lane,
    write: Lane,
    control: Lane,
    stopping: Arc<AtomicBool>,
    order: Mutex<MutationOrder>,
    artifact_tasks: Mutex<JoinSet<()>>,
}

impl RpcRequestDispatcher {
    pub(crate) fn new(outbound: crate::transport::OutboundSender) -> Self {
        Self::with_limits(
            outbound,
            READ_CAPACITY,
            READ_CONCURRENCY,
            WRITE_CAPACITY,
            CONTROL_CAPACITY,
        )
    }

    fn with_limits(
        outbound: crate::transport::OutboundSender,
        read_capacity: usize,
        read_concurrency: usize,
        write_capacity: usize,
        control_capacity: usize,
    ) -> Self {
        let stopping = Arc::new(AtomicBool::new(false));
        Self {
            read: Lane::new(
                RpcDispatchClass::Read,
                read_capacity,
                read_concurrency,
                Arc::clone(&stopping),
                outbound.clone(),
            ),
            write: Lane::new(
                RpcDispatchClass::Write,
                write_capacity,
                1,
                Arc::clone(&stopping),
                outbound.clone(),
            ),
            control: Lane::new(
                RpcDispatchClass::Control,
                control_capacity,
                2,
                Arc::clone(&stopping),
                outbound,
            ),
            stopping,
            order: Mutex::new(MutationOrder::default()),
            artifact_tasks: Mutex::new(JoinSet::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn try_submit<F>(
        &self,
        class: RpcDispatchClass,
        request_id: JsonRpcId,
        owner: Option<String>,
        task: F,
    ) -> Result<(), Value>
    where
        F: FnOnce() -> Value + Send + 'static,
    {
        self.try_submit_with_owner_resolution(class, request_id, owner, None, task)
    }

    pub(crate) fn try_submit_with_owner_resolution<F>(
        &self,
        class: RpcDispatchClass,
        request_id: JsonRpcId,
        owner: Option<String>,
        owner_resolver: Option<RpcOwnerResolver>,
        task: F,
    ) -> Result<(), Value>
    where
        F: FnOnce() -> Value + Send + 'static,
    {
        if self.stopping.load(Ordering::Acquire) {
            return Err(unavailable_response(request_id, false));
        }
        let lane = match class {
            RpcDispatchClass::Read => &self.read,
            RpcDispatchClass::Write => &self.write,
            RpcDispatchClass::Control => &self.control,
        };
        let permit = Arc::clone(&lane.admission)
            .try_acquire_owned()
            .map_err(|_| {
                unavailable_response(request_id.clone(), !self.stopping.load(Ordering::Acquire))
            })?;
        let mut order = self.order.lock().unwrap_or_else(|error| error.into_inner());
        // Retain only outstanding owners, bounded by admitted mutations rather than host uptime.
        order
            .owners
            .retain(|_, completion| !*completion.borrow() && completion.has_changed().is_ok());
        order
            .controls
            .retain(|completion| !*completion.borrow() && completion.has_changed().is_ok());
        let dependencies = match class {
            RpcDispatchClass::Control => owner
                .as_ref()
                .and_then(|owner| order.owners.get(owner))
                .cloned()
                .into_iter()
                .collect(),
            _ => order
                .last_write
                .iter()
                .chain(order.controls.iter())
                .cloned()
                .collect(),
        };
        let owner_resolution = owner_resolver.and_then(|resolve| {
            let candidates = order
                .owners
                .iter()
                .filter(|(owner, _)| owner.starts_with("conversation:"))
                .map(|(owner, completion)| (owner.clone(), completion.clone()))
                .collect::<HashMap<_, _>>();
            (!candidates.is_empty()).then_some(OwnerResolution {
                resolve,
                candidates,
            })
        });
        let (completion, receiver) = if class != RpcDispatchClass::Read {
            let (sender, receiver) = watch::channel(false);
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        lane.sender
            .try_send(RpcJob {
                request_id: request_id.clone(),
                task: Box::new(task),
                admitted_at: std::time::Instant::now(),
                dependencies,
                owner_resolution,
                completion,
                _permit: permit,
            })
            .map_err(|error| {
                unavailable_response(
                    request_id,
                    matches!(error, mpsc::error::TrySendError::Full(_)),
                )
            })?;
        if let Some(receiver) = receiver {
            match class {
                RpcDispatchClass::Write => order.last_write = Some(receiver.clone()),
                RpcDispatchClass::Control => order.controls.push(receiver.clone()),
                RpcDispatchClass::Read => unreachable!(),
            }
            if let Some(owner) = owner {
                order.owners.insert(owner, receiver);
            }
        }
        Ok(())
    }

    // Artifact admission is independently bounded by permits held through writer flush.
    // Keep its task owner here so accepted artifact responses also precede writer shutdown.
    pub(crate) fn track_artifact(
        &self,
        task: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let mut tasks = self
            .artifact_tasks
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while tasks.try_join_next().is_some() {}
        tasks.spawn(task);
    }

    pub(crate) fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        self.read.admission.close();
        self.write.admission.close();
        self.control.admission.close();
    }

    pub(crate) async fn shutdown(self) -> io::Result<()> {
        // Do not cancel queued requests on EOF. The explicit shutdown boundary calls
        // begin_shutdown itself; transport failure/EOF still drains everything already accepted.
        let (read, write, control) = tokio::join!(
            self.read.shutdown(),
            self.write.shutdown(),
            self.control.shutdown()
        );
        let mut artifacts = self
            .artifact_tasks
            .into_inner()
            .unwrap_or_else(|error| error.into_inner());
        let mut artifact_result = Ok(());
        while let Some(result) = artifacts.join_next().await {
            if let Err(error) = result {
                artifact_result = Err(io::Error::other(format!(
                    "artifact request task failed: {error}"
                )));
            }
        }
        read.and(write).and(control).and(artifact_result)
    }
}

pub(super) fn unavailable_response(id: JsonRpcId, busy: bool) -> Value {
    serde_json::to_value(mycopilot_protocol_rs::error_with_data(
        Some(id), if busy { -32001 } else { -32603 },
        if busy { "RPC request queue is full. Retry shortly." } else { "RPC request was not started because the core server is shutting down." },
        json!({"type":"rpc_dispatch_error", "code": if busy { "overloaded" } else { "shutting_down" }, "retryable":true}),
    )).expect("RPC admission errors serialize")
}

pub(super) fn is_blocking_read_method(method: &str) -> bool {
    matches!(
        method,
        AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD
            | mycopilot_protocol_rs::AGENT_COLLABORATION_GET_SETTINGS_METHOD
            | AGENT_COLLABORATION_GET_TREE_METHOD
            | AGENT_COLLABORATION_GET_AGENT_METHOD
            | AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD
            | AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD
            | AGENT_COLLABORATION_LIST_EVENTS_METHOD
            | AGENT_COLLABORATION_TEMPLATES_LIST_METHOD
            | AGENT_COLLABORATION_APPROVALS_LIST_METHOD
            | AGENT_COMMAND_SESSIONS_LIST_METHOD
            | AGENT_COMMAND_SESSIONS_GET_METHOD
            | AGENT_LIST_PENDING_ACTIONS_METHOD
            | AGENT_GET_USAGE_SUMMARY_METHOD
            | AGENT_GET_LOCAL_TOKEN_USAGE_METHOD
            | AGENT_READ_FILE_CHANGE_METHOD
            | AGENT_GET_FILE_CHANGE_DIFF_METHOD
            | AGENT_GET_FILE_CHANGE_HISTORY_DIFF_METHOD
            | SEARCH_SEARCH_CHATS_METHOD
            | STORAGE_LOAD_MODEL_SETTINGS_METHOD
            | STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD
            | STORAGE_LOAD_PROVIDER_VENDOR_DESCRIPTORS_METHOD
            | STORAGE_RESOLVE_PROVIDER_VENDOR_MODEL_POLICY_METHOD
            | STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD
            | STORAGE_LOAD_PROJECTS_METHOD
            | STORAGE_LOAD_CONVERSATIONS_METHOD
            | STORAGE_LOAD_CONVERSATION_METAS_METHOD
            | mycopilot_protocol_rs::STORAGE_LOAD_RUNNING_CONVERSATION_SUMMARIES_METHOD
            | mycopilot_protocol_rs::HUMAN_INTERACTION_GET_ATTENTION_METHOD
            | STORAGE_LOAD_CONVERSATION_METHOD
            | STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD
            | STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD
            | mycopilot_protocol_rs::STORAGE_LOAD_INPUT_ATTACHMENT_PREVIEW_METHOD
            | STORAGE_LOAD_BROWSER_DOWNLOAD_SETTINGS_METHOD
            | STORAGE_LIST_BROWSER_DOWNLOADS_METHOD
            | STORAGE_LOAD_BROWSER_DOWNLOAD_METHOD
            | STORAGE_LOAD_COMPOSER_DRAFTS_METHOD
            | STORAGE_LOAD_UI_PREFERENCES_METHOD
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as sync_mpsc;

    fn id(value: i64) -> JsonRpcId {
        JsonRpcId::Number(value)
    }
    fn ok(value: i64) -> Value {
        response_success(id(value), true)
    }

    #[test]
    fn operations_are_classified_without_promoting_writes_to_reads() {
        let class = |method: &str, operation: Option<&str>| {
            rpc_dispatch_class(&JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: id(1),
                method: method.into(),
                params: operation.map(|operation| json!({"operation":operation})),
            })
        };
        for operation in ["list", "listInstances", "validate", "runtimeSnapshot"] {
            assert_eq!(
                class("agent.workflows.request", Some(operation)),
                RpcDispatchClass::Read
            );
        }
        for operation in [
            "save",
            "saveInstance",
            "deleteInstance",
            "setInstanceEnabled",
            "saveDraft",
            "deleteDraft",
            "duplicate",
            "unknown",
        ] {
            assert_eq!(
                class("agent.workflows.request", Some(operation)),
                RpcDispatchClass::Write
            );
        }
        for method in [
            mycopilot_protocol_rs::STORAGE_BEGIN_ATTACHMENT_IMPORT_METHOD,
            mycopilot_protocol_rs::STORAGE_APPEND_ATTACHMENT_IMPORT_METHOD,
            mycopilot_protocol_rs::STORAGE_FINISH_ATTACHMENT_IMPORT_METHOD,
            mycopilot_protocol_rs::STORAGE_CANCEL_ATTACHMENT_IMPORT_METHOD,
            mycopilot_protocol_rs::HUMAN_INTERACTION_SUBMIT_METHOD,
            mycopilot_protocol_rs::HUMAN_INTERACTION_IGNORE_METHOD,
            mycopilot_protocol_rs::HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD,
            mycopilot_protocol_rs::NOTIFICATION_BATCH_VALIDATE_METHOD,
            mycopilot_protocol_rs::CORE_SET_EXECUTION_ACCESS_METHOD,
        ] {
            assert_eq!(class(method, None), RpcDispatchClass::Write, "{method}");
        }
        for method in [
            mycopilot_protocol_rs::HUMAN_INTERACTION_GET_SETTINGS_METHOD,
            mycopilot_protocol_rs::HUMAN_INTERACTION_LIST_REQUESTS_METHOD,
            mycopilot_protocol_rs::HUMAN_INTERACTION_GET_ATTENTION_METHOD,
            mycopilot_protocol_rs::STORAGE_LOAD_RUNNING_CONVERSATION_SUMMARIES_METHOD,
        ] {
            assert_eq!(class(method, None), RpcDispatchClass::Read, "{method}");
        }
        for method in [
            AGENT_CANCEL_RUN_METHOD,
            AGENT_CANCEL_ACTION_METHOD,
            mycopilot_protocol_rs::AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD,
        ] {
            assert_eq!(class(method, None), RpcDispatchClass::Control, "{method}");
        }
    }

    #[test]
    fn run_attachment_file_resolution_is_a_read() {
        assert_eq!(
            rpc_dispatch_class(&JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: id(1),
                method: "storage.resolveRunAttachmentFile".into(),
                params: None,
            }),
            RpcDispatchClass::Read
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn saturated_reads_and_unrelated_writes_do_not_block_control_admission() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::with_limits(outbound, 2, 1, 2, 2);
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        rpc.try_submit(RpcDispatchClass::Read, id(1), None, move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            ok(1)
        })
        .unwrap();
        ready.await.unwrap();
        rpc.try_submit(RpcDispatchClass::Read, id(2), None, || ok(2))
            .unwrap();
        let full = rpc
            .try_submit(RpcDispatchClass::Read, id(3), None, || unreachable!())
            .unwrap_err();
        assert_eq!(full["id"], 3);
        assert_eq!(full["error"]["code"], -32001);
        assert_eq!(full["error"]["data"]["code"], "overloaded");
        let (write_started, write_ready) = oneshot::channel();
        let (write_release, write_wait) = sync_mpsc::channel();
        rpc.try_submit(
            RpcDispatchClass::Write,
            id(4),
            Some("conversation:A".into()),
            move || {
                write_started.send(()).unwrap();
                write_wait.recv().unwrap();
                ok(4)
            },
        )
        .unwrap();
        write_ready.await.unwrap();
        rpc.try_submit(
            RpcDispatchClass::Control,
            id(5),
            Some("run:B".into()),
            || ok(5),
        )
        .unwrap();
        let control = tokio::time::timeout(Duration::from_millis(500), responses.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            control["id"], 5,
            "control must bypass unrelated blocked work"
        );
        release.send(()).unwrap();
        write_release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        let mut ids = vec![];
        while let Some(response) = responses.recv().await {
            ids.push(response["id"].as_i64().unwrap());
        }
        ids.sort();
        assert_eq!(ids, vec![1, 2, 4]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mutation_order_and_read_after_write_hold_across_owner_keys() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let state = Arc::new(Mutex::new(vec![]));
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        let first = Arc::clone(&state);
        rpc.try_submit(
            RpcDispatchClass::Write,
            id(1),
            Some("instance:A".into()),
            move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                first.lock().unwrap().push(1);
                ok(1)
            },
        )
        .unwrap();
        ready.await.unwrap();
        let second = Arc::clone(&state);
        rpc.try_submit(
            RpcDispatchClass::Write,
            id(2),
            Some("instance:B".into()),
            move || {
                second.lock().unwrap().push(2);
                ok(2)
            },
        )
        .unwrap();
        let read = Arc::clone(&state);
        rpc.try_submit(RpcDispatchClass::Read, id(3), None, move || {
            response_success(id(3), read.lock().unwrap().clone())
        })
        .unwrap();
        assert!(responses.try_recv().is_err());
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        let mut result = None;
        while let Some(response) = responses.recv().await {
            if response["id"] == 3 {
                result = Some(response["result"].clone());
            }
        }
        assert_eq!(result, Some(json!([1, 2])));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_waits_for_same_owner_admission_and_later_writes_wait_for_cancel() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let state = Arc::new(Mutex::new(vec![]));
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        let start_state = Arc::clone(&state);
        rpc.try_submit(
            RpcDispatchClass::Write,
            id(1),
            Some("conversation:A".into()),
            move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                start_state.lock().unwrap().push("start");
                ok(1)
            },
        )
        .unwrap();
        ready.await.unwrap();
        let cancel_state = Arc::clone(&state);
        rpc.try_submit(
            RpcDispatchClass::Control,
            id(2),
            Some("conversation:A".into()),
            move || {
                cancel_state.lock().unwrap().push("cancel");
                ok(2)
            },
        )
        .unwrap();
        let later_state = Arc::clone(&state);
        rpc.try_submit(
            RpcDispatchClass::Write,
            id(3),
            Some("conversation:B".into()),
            move || {
                assert_eq!(*later_state.lock().unwrap(), ["start", "cancel"]);
                ok(3)
            },
        )
        .unwrap();
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        let mut ids = vec![];
        while let Some(response) = responses.recv().await {
            ids.push(response["id"].as_i64().unwrap());
        }
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_database_lock_remains_visible_after_control_dispatch() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let database = Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        let db = Arc::clone(&database);
        rpc.try_submit(RpcDispatchClass::Read, id(1), None, move || {
            let _lock = db.lock().unwrap();
            started.send(()).unwrap();
            wait.recv().unwrap();
            ok(1)
        })
        .unwrap();
        ready.await.unwrap();
        let (attempting, attempted) = oneshot::channel();
        rpc.try_submit(RpcDispatchClass::Control, id(2), None, move || {
            attempting.send(()).unwrap();
            let db = database.lock().unwrap();
            let value = db
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap();
            response_success(id(2), value)
        })
        .unwrap();
        attempted.await.unwrap();
        assert!(
            responses.try_recv().is_err(),
            "control is dispatched but still awaits the shared DB mutex"
        );
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        let mut ids = vec![];
        while let Some(response) = responses.recv().await {
            ids.push(response["id"].as_i64().unwrap());
        }
        ids.sort();
        assert_eq!(ids, vec![1, 2]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_joins_running_mutation_and_responds_to_every_cancelled_queue_entry() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::with_limits(outbound, 2, 1, 3, 2);
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        rpc.try_submit(RpcDispatchClass::Write, id(1), None, move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            ok(1)
        })
        .unwrap();
        ready.await.unwrap();
        for number in 2..=3 {
            rpc.try_submit(RpcDispatchClass::Write, id(number), None, || {
                panic!("queued mutation must not start")
            })
            .unwrap();
        }
        rpc.begin_shutdown();
        let rejected = rpc
            .try_submit(RpcDispatchClass::Read, id(4), None, || unreachable!())
            .unwrap_err();
        assert_eq!(rejected["error"]["data"]["code"], "shutting_down");
        let stopping = tokio::spawn(rpc.shutdown());
        tokio::task::yield_now().await;
        assert!(!stopping.is_finished());
        release.send(()).unwrap();
        stopping.await.unwrap().unwrap();
        let first = responses.recv().await.unwrap();
        assert_eq!(first["result"], true);
        for number in 2..=3 {
            let response = responses.recv().await.unwrap();
            assert_eq!(response["id"], number);
            assert_eq!(response["error"]["data"]["code"], "shutting_down");
        }
        assert!(responses.recv().await.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn worker_failure_keeps_original_id_and_releases_ordering_fences() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        rpc.try_submit(RpcDispatchClass::Write, id(1), None, || {
            panic!("injected handler failure")
        })
        .unwrap();
        rpc.try_submit(RpcDispatchClass::Read, id(2), None, || ok(2))
            .unwrap();
        rpc.shutdown().await.unwrap();
        let failure = responses.recv().await.unwrap();
        assert_eq!(failure["id"], 1);
        assert_eq!(failure["error"]["code"], -32603);
        let success = responses.recv().await.unwrap();
        assert_eq!(success["id"], 2);
        assert_eq!(success["result"], true);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn waiting_owner_control_does_not_block_another_owner_or_lose_write_fences() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        rpc.try_submit(
            RpcDispatchClass::Write,
            id(1),
            Some("run:A".into()),
            move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                ok(1)
            },
        )
        .unwrap();
        ready.await.unwrap();
        let cancelled_a = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&cancelled_a);
        rpc.try_submit(
            RpcDispatchClass::Control,
            id(2),
            Some("run:A".into()),
            move || {
                cancelled.store(true, Ordering::Release);
                ok(2)
            },
        )
        .unwrap();
        rpc.try_submit(
            RpcDispatchClass::Control,
            id(3),
            Some("run:B".into()),
            || ok(3),
        )
        .unwrap();
        let first = tokio::time::timeout(Duration::from_millis(500), responses.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first["id"], 3, "B must not wait for A's owner dependency");
        rpc.try_submit(RpcDispatchClass::Write, id(4), None, move || {
            assert!(
                cancelled_a.load(Ordering::Acquire),
                "a later completed B must not hide pending A"
            );
            ok(4)
        })
        .unwrap();
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        let mut ids = vec![];
        while let Some(response) = responses.recv().await {
            ids.push(response["id"].as_i64().unwrap());
        }
        assert_eq!(ids, vec![1, 2, 4]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn control_capacity_counts_waiting_and_running_requests() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::with_limits(outbound, 1, 1, 1, 2);
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        rpc.try_submit(
            RpcDispatchClass::Write,
            id(1),
            Some("run:A".into()),
            move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                ok(1)
            },
        )
        .unwrap();
        ready.await.unwrap();
        for number in 2..=3 {
            rpc.try_submit(
                RpcDispatchClass::Control,
                id(number),
                Some("run:A".into()),
                move || ok(number),
            )
            .unwrap();
        }
        let full = rpc
            .try_submit(
                RpcDispatchClass::Control,
                id(4),
                Some("run:B".into()),
                || unreachable!(),
            )
            .unwrap_err();
        assert_eq!(full["error"]["data"]["code"], "overloaded");
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        let mut ids = vec![];
        while let Some(response) = responses.recv().await {
            ids.push(response["id"].as_i64().unwrap());
        }
        assert_eq!(ids, vec![1, 2, 3]);
    }
    #[test]
    fn single_owner_mutation_dtos_keep_their_class_and_ordering_identity() {
        use mycopilot_protocol_rs::*;
        for method in [
            AGENT_START_CONVERSATION_TURN_METHOD,
            AGENT_CONTINUE_CONVERSATION_TURN_METHOD,
            AGENT_START_PROVIDER_TRANSITION_METHOD,
            AGENT_START_MANUAL_CONTEXT_COMPACTION_METHOD,
            AGENT_STEER_RUN_METHOD,
            HUMAN_INTERACTION_SUBMIT_METHOD,
            HUMAN_INTERACTION_IGNORE_METHOD,
            STORAGE_DELETE_CONVERSATION_METHOD,
            STORAGE_DELETE_CHAT_MESSAGES_METHOD,
            STORAGE_UPSERT_CHAT_MESSAGES_METHOD,
            STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD,
            STORAGE_SAVE_CHAT_MESSAGE_UI_STATE_METHOD,
        ] {
            let request = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: id(1),
                method: method.into(),
                params: Some(json!({"conversationId":"c", "expectedRunId":"r"})),
            };
            assert_eq!(
                rpc_dispatch_class(&request),
                RpcDispatchClass::Write,
                "{method}"
            );
            assert_eq!(
                rpc_ordering_key(&request),
                Some("conversation:c".into()),
                "{method}"
            );
        }
        for (method, params, class, owner) in [
            (
                AGENT_APPROVE_ACTION_METHOD,
                json!({"runId":"r", "actionId":"a"}),
                RpcDispatchClass::Write,
                "run:r",
            ),
            (
                AGENT_REJECT_ACTION_METHOD,
                json!({"runId":"r", "actionId":"a"}),
                RpcDispatchClass::Write,
                "run:r",
            ),
            (
                AGENT_CANCEL_ACTION_METHOD,
                json!({"runId":"r", "actionId":"a"}),
                RpcDispatchClass::Control,
                "run:r",
            ),
            (
                AGENT_CANCEL_RUN_METHOD,
                json!({"runId":"r"}),
                RpcDispatchClass::Control,
                "run:r",
            ),
            (
                AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD,
                json!({"conversationId":"c", "operationId":"o"}),
                RpcDispatchClass::Control,
                "conversation:c",
            ),
            (
                AGENT_REWRITE_CONVERSATION_TURN_METHOD,
                json!({"turn":{"conversationId":"c"}}),
                RpcDispatchClass::Write,
                "conversation:c",
            ),
            (
                AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD,
                json!({"rootConversationId":"c", "approvalId":"a"}),
                RpcDispatchClass::Write,
                "conversation:c",
            ),
            (
                STORAGE_SAVE_CONVERSATION_META_METHOD,
                json!({"id":"c"}),
                RpcDispatchClass::Write,
                "conversation:c",
            ),
            (
                STORAGE_SAVE_COMPOSER_DRAFT_METHOD,
                json!({"draft":{"scopeId":"c"}}),
                RpcDispatchClass::Write,
                "conversation:c",
            ),
            (
                STORAGE_SAVE_COMPOSER_DRAFT_MESSAGE_METHOD,
                json!({"scopeId":"c"}),
                RpcDispatchClass::Write,
                "conversation:c",
            ),
            (
                AUTOMATION_CREATE_METHOD,
                json!({"destination":{"kind":"existing_chat", "conversationId":"c"}}),
                RpcDispatchClass::Write,
                "conversation:c",
            ),
            (
                AUTOMATION_UPDATE_METHOD,
                json!({"destination":{"kind":"existing_chat", "conversationId":"c"}}),
                RpcDispatchClass::Write,
                "conversation:c",
            ),
            (
                STORAGE_REGISTER_BROWSER_DOWNLOAD_METHOD,
                json!({"runId":"r", "conversationId":"c"}),
                RpcDispatchClass::Write,
                "conversation:c",
            ),
            (
                STORAGE_REGISTER_BROWSER_DOWNLOAD_METHOD,
                json!({"runId":"r", "conversationId":null}),
                RpcDispatchClass::Write,
                "run:r",
            ),
        ] {
            let request = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: id(1),
                method: method.into(),
                params: Some(params),
            };
            assert_eq!(rpc_dispatch_class(&request), class, "{method}");
            assert_eq!(rpc_ordering_key(&request), Some(owner.into()), "{method}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_to_conversation_alias_preserves_submission_before_cancel_without_blocking_other_runs(
    ) {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        rpc.try_submit(
            RpcDispatchClass::Write,
            id(1),
            Some("conversation:A".into()),
            move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                ok(1)
            },
        )
        .unwrap();
        ready.await.unwrap();
        let alias = |conversation: &'static str| {
            Some(Box::new(move || Some(format!("conversation:{conversation}"))) as RpcOwnerResolver)
        };
        rpc.try_submit_with_owner_resolution(
            RpcDispatchClass::Control,
            id(2),
            Some("run:A".into()),
            alias("A"),
            || ok(2),
        )
        .unwrap();
        rpc.try_submit_with_owner_resolution(
            RpcDispatchClass::Control,
            id(3),
            Some("run:B".into()),
            alias("B"),
            || ok(3),
        )
        .unwrap();
        let first = tokio::time::timeout(Duration::from_millis(500), responses.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first["id"], 3,
            "unrelated run must bypass conversation A's earlier human submission"
        );
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        assert_eq!(responses.recv().await.unwrap()["id"], 1);
        assert_eq!(
            responses.recv().await.unwrap()["id"],
            2,
            "same conversation cancellation must follow the accepted submission"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_drains_started_owner_resolution_and_skips_waiting_resolvers_and_handlers() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (write_started, write_ready) = oneshot::channel();
        let (write_release, write_wait) = sync_mpsc::channel();
        rpc.try_submit(
            RpcDispatchClass::Write,
            id(1),
            Some("conversation:A".into()),
            move || {
                write_started.send(()).unwrap();
                write_wait.recv().unwrap();
                ok(1)
            },
        )
        .unwrap();
        write_ready.await.unwrap();
        let mut releases = vec![];
        for number in 2..=3 {
            let (started, ready) = oneshot::channel();
            let (release, wait) = sync_mpsc::channel();
            releases.push(release);
            rpc.try_submit_with_owner_resolution(
                RpcDispatchClass::Control,
                id(number),
                Some(format!("run:{number}")),
                Some(Box::new(move || {
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    Some("conversation:A".into())
                })),
                || panic!("shutdown must skip handler after resolving the owner"),
            )
            .unwrap();
            ready.await.unwrap();
        }
        rpc.try_submit_with_owner_resolution(
            RpcDispatchClass::Control,
            id(4),
            Some("run:4".into()),
            Some(Box::new(|| {
                panic!("shutdown must skip a resolver waiting for execution capacity")
            })),
            || unreachable!(),
        )
        .unwrap();
        tokio::task::yield_now().await;
        rpc.begin_shutdown();
        let shutdown = tokio::spawn(rpc.shutdown());
        write_release.send(()).unwrap();
        assert_eq!(responses.recv().await.unwrap()["id"], 1);
        assert!(
            !shutdown.is_finished(),
            "running owner resolvers must retain their owner"
        );
        for release in releases {
            release.send(()).unwrap();
        }
        shutdown.await.unwrap().unwrap();
        let mut ids = vec![];
        while let Some(response) = responses.recv().await {
            assert_eq!(response["error"]["data"]["code"], "shutting_down");
            ids.push(response["id"].as_i64().unwrap());
        }
        ids.sort();
        assert_eq!(ids, [2, 3, 4]);
    }

    #[test]
    fn shutdown_does_not_start_handlers_still_queued_in_the_shared_blocking_pool() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (pool_started, pool_ready) = oneshot::channel();
            let (pool_release, pool_wait) = sync_mpsc::channel();
            let occupancy = tokio::task::spawn_blocking(move || {
                pool_started.send(()).unwrap();
                pool_wait.recv().unwrap();
            });
            pool_ready.await.unwrap();
            let (outbound, mut responses) = crate::transport::outbound_channel();
            let rpc = RpcRequestDispatcher::new(outbound);
            rpc.try_submit(RpcDispatchClass::Write, id(1), None, || {
                panic!("queued blocking handler must not start after shutdown")
            })
            .unwrap();
            // Let the lane schedule its blocking task while the only pool thread is occupied.
            tokio::time::sleep(Duration::from_millis(20)).await;
            rpc.begin_shutdown();
            pool_release.send(()).unwrap();
            occupancy.await.unwrap();
            rpc.shutdown().await.unwrap();
            let response = responses.recv().await.unwrap();
            assert_eq!(response["id"], 1);
            assert_eq!(response["error"]["data"]["code"], "shutting_down");
        });
    }
}

#[cfg(test)]
#[path = "bounded_dispatch_benchmarks.rs"]
mod benchmarks;
