//! Opt-in, disk-backed, real RPC/AgentService baseline. Never opens application data or a
//! remote model. Timings begin at scheduled Core intake, not at a renderer click.
//! Release tests enable Core's `test-support` dev-dependency feature solely for the controlled
//! loopback HTTP provider. Normal production builds do not enable this feature and require HTTPS.
use super::managed_command_loop::{read_json_request, write_text_stream, write_tool_call_stream};
use super::*;
use crate::transport::baseline_dispatch::BaselineDispatcher;
use mycopilot_protocol_rs::{
    ExecutionAccessReason, JsonRpcRequest, SetExecutionAccessInput, AGENT_APPROVE_ACTION_METHOD,
    AGENT_CANCEL_RUN_METHOD, AGENT_START_CONVERSATION_TURN_METHOD,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinSet;

// Fixed for every case. The shared test model's 128k window triggers automatic compaction for
// this 128-pair history after production output/safety reserves and fixed tools are included.
// Compaction is a separate workload; never let its provider calls contaminate these timings.
const FIXTURE_CONTEXT_WINDOW_TOKENS: u32 = 256_000;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Work {
    Text,
    SteadyText,
    Mixed,
    Approval,
    ApprovalBehindPreparation,
    Stop,
    SlowTool,
    Tree,
}

#[derive(Clone, Debug, Serialize)]
struct Case {
    units: usize,
    roots: usize,
    history_pairs: usize,
    work: Work,
    slow_seconds: u64,
    waves: usize,
}
impl Case {
    fn name(&self) -> String {
        format!(
            "{:?}-u{}-r{}-h{}-s{}",
            self.work, self.units, self.roots, self.history_pairs, self.slow_seconds
        )
        .to_ascii_lowercase()
    }
}

#[derive(Clone, Serialize)]
struct RpcSample {
    id: i64,
    owner: usize,
    operation: &'static str,
    phase: &'static str,
    provider_key: Option<String>,
    scheduled_us: u64,
    offered_us: u64,
    handler_started_us: Option<u64>,
    responded_us: Option<u64>,
    dispatch_admitted: bool,
    response: Option<Value>,
    response_bytes: usize,
}
#[derive(Default, Serialize)]
struct Events {
    rpcs: BTreeMap<i64, RpcSample>,
    terminal: BTreeMap<String, Value>,
    command_started_us: BTreeMap<String, u64>,
    command_exited_us: BTreeMap<String, u64>,
    context_peak_by_run: BTreeMap<String, Value>,
    context_compaction_events: usize,
    execution_access_grants: Vec<Value>,
    provider_first_us: BTreeMap<String, u64>,
    provider_requests: usize,
    provider_request_bytes: usize,
    output_events: usize,
    output_bytes: usize,
    errors: Vec<Value>,
    timer_lag_us: Vec<u64>,
    resource_samples: Vec<Value>,
    #[serde(skip)]
    ready_stops: Vec<(usize, String, String)>,
}
fn us(elapsed: Duration) -> u64 {
    elapsed.as_micros().min(u64::MAX as u128) as u64
}
fn sample(events: &Arc<Mutex<Events>>, update: impl FnOnce(&mut Events)) {
    update(&mut events.lock().unwrap_or_else(|e| e.into_inner()));
}
fn quantiles(mut values: Vec<u64>) -> Value {
    values.sort_unstable();
    let at = |p: f64| {
        values
            .get(((values.len() as f64 * p).ceil() as usize).saturating_sub(1))
            .copied()
    };
    json!({"count":values.len(),"p50_us":at(0.50),"p95_us":at(0.95),"p99_us":at(0.99),
        "max_us":values.last(),"p99_reliable":values.len() >= 10_000})
}
fn process_io() -> Value {
    #[cfg(target_os = "macos")]
    {
        // Local SDK libproc.h / sys/resource.h, RUSAGE_INFO_V4. Counts belong to this process,
        // not the entire device, and never stand in for fsync latency or NAND writes.
        unsafe extern "C" {
            fn proc_pid_rusage(
                pid: libc::c_int,
                flavor: libc::c_int,
                buffer: *mut *mut libc::c_void,
            ) -> libc::c_int;
        }
        let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
        if unsafe {
            proc_pid_rusage(
                std::process::id() as libc::c_int,
                4,
                info.as_mut_ptr().cast(),
            )
        } == 0
        {
            let info = unsafe { info.assume_init() };
            return json!({"source":"proc_pid_rusage_v4","diskio_bytes_read":info.ri_diskio_bytesread,
                "diskio_bytes_written":info.ri_diskio_byteswritten,"logical_writes":info.ri_logical_writes,
                "pageins":info.ri_pageins,"resident_bytes":info.ri_resident_size,"physical_footprint_bytes":info.ri_phys_footprint});
        }
    }
    Value::Null
}

fn usage() -> Value {
    // getrusage maxrss is a process lifetime high-water mark, NOT per-case allocation.
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0;
    if !ok {
        return Value::Null;
    }
    let usage = unsafe { usage.assume_init() };
    let time = |value: libc::timeval| value.tv_sec as f64 + value.tv_usec as f64 / 1_000_000.0;
    json!({"cpu_user_seconds":time(usage.ru_utime),"cpu_system_seconds":time(usage.ru_stime),
        "process_lifetime_maxrss_raw":usage.ru_maxrss,
        "maxrss_unit":if cfg!(target_os="macos") {"bytes"} else {"KiB"},
        "process_io":process_io(),"block_output_operations":usage.ru_oublock,"block_input_operations":usage.ru_inblock,
        "voluntary_context_switches":usage.ru_nvcsw,"involuntary_context_switches":usage.ru_nivcsw})
}
fn disk_files(path: &Path) -> Value {
    let size = |suffix: &str| {
        std::fs::metadata(format!("{}{suffix}", path.display()))
            .map(|m| m.len())
            .unwrap_or(0)
    };
    json!({"database_bytes":size(""),"wal_bytes":size("-wal"),"journal_bytes":size("-journal"),"shm_bytes":size("-shm")})
}
fn sqlite_info(path: &Path) -> Value {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    json!({"version":rusqlite::version(),
        "journal_mode":connection.query_row("PRAGMA journal_mode",[],|r|r.get::<_,String>(0)).unwrap(),
        "synchronous_on_inspection_connection":connection.query_row("PRAGMA synchronous",[],|r|r.get::<_,i64>(0)).unwrap(),
        "user_version":connection.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap()})
}
fn seed(storage: &StorageService, units: usize, pairs: usize) {
    for unit in 0..units {
        let mut messages = Vec::new();
        for pair in 0..pairs {
            for (role, content) in [
                ("user", "Synthetic historical requirement. ".repeat(16)),
                ("assistant", "Synthetic completed answer. ".repeat(16)),
            ] {
                messages.push(ChatMessageRecord {
                    id: format!("history-{unit}-{pair}-{role}"),
                    role: role.into(),
                    content,
                    created_at: (pair * 2 + usize::from(role == "assistant") + 1) as i64,
                    status: Some("sent".into()),
                    human_interaction_response: None,
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                });
            }
        }
        storage
            .save_conversation(ChatConversationRecord {
                id: format!("baseline-{unit}"),
                project_id: Some("baseline-project".into()),
                model_id: Some("model-1".into()),
                title: format!("Synthetic unit {unit}"),
                messages,
                created_at: 1,
                updated_at: (pairs * 2 + 1) as i64,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        // Give every historical assistant message an authoritative terminal Trace. Do not use
        // malformed imported history to make preparation artificially cheap or fail early.
        for pair in 0..pairs {
            let conversation = format!("baseline-{unit}");
            let assistant = format!("history-{unit}-{pair}-assistant");
            storage
                .finalize_chat_message_with_conversation_trace(
                    &conversation,
                    &assistant,
                    &"Synthetic completed answer. ".repeat(16),
                    Some("sent"),
                    "completed",
                    &completed_conversation_trace_without_items(
                        &format!("history-run-{unit}-{pair}"),
                        &conversation,
                        &assistant,
                    ),
                    (pair * 2 + 2) as i64,
                    (pair * 2 + 2) as i64,
                )
                .unwrap();
        }
    }
}
fn text_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(text_content)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map.get("text").map(text_content).unwrap_or_default(),
        _ => String::new(),
    }
}
fn marker(request: &Value) -> String {
    let messages = request["messages"].as_array().unwrap();
    for message in messages.iter().rev().filter(|m| m["role"] == "user") {
        let text = text_content(&message["content"]);
        if let Some((_, tail)) = text.rsplit_once("BASELINE:") {
            return tail
                .split_whitespace()
                .next()
                .unwrap()
                .trim_matches(|c| c == '"' || c == '<' || c == '>')
                .to_string();
        }
    }
    "unknown".into()
}
fn current_tool_results(request: &Value, current_turn_only: bool) -> Vec<Value> {
    let messages = request["messages"].as_array().unwrap();
    let first = if current_turn_only {
        messages
            .iter()
            .rposition(|message| {
                message["role"] == "user" && text_content(&message["content"]).contains("BASELINE:")
            })
            .map_or(0, |index| index + 1)
    } else {
        0
    };
    messages[first..]
        .iter()
        .filter(|message| message["role"] == "tool")
        .filter_map(|message| message["content"].as_str())
        .filter_map(|text| serde_json::from_str(text).ok())
        .collect()
}

async fn provider_response(stream: &mut tokio::net::TcpStream, request: &Value, case: &Case) {
    let unit = marker(request);
    let owner = unit
        .split('-')
        .nth(1)
        .and_then(|part| part.parse::<usize>().ok())
        .unwrap_or(0);
    let wave = unit
        .split("-wave-")
        .nth(1)
        .and_then(|part| part.parse::<usize>().ok())
        .unwrap_or(0);
    let mixed = matches!(case.work, Work::Mixed);
    let needs_approval = mixed && wave % 5 == 3
        || matches!(case.work, Work::Approval)
        || matches!(case.work, Work::ApprovalBehindPreparation) && owner == 0
        || matches!(case.work, Work::SlowTool) && owner != 0;
    let tool_results = current_tool_results(request, mixed);
    if matches!(case.work, Work::Tree) && !unit.contains("child") && tool_results.is_empty() {
        let root = unit.split('-').nth(1).unwrap().parse::<usize>().unwrap();
        let children = (case.units - case.roots) / case.roots
            + usize::from(root < (case.units - case.roots) % case.roots);
        let calls=(0..children).map(|child|json!({"index":child,"id":format!("spawn-{child}"),"type":"function",
            "function":{"name":"spawn_agent","arguments":serde_json::to_string(&json!({
                "task_name":format!("child_{child}"),"message":format!("Complete synthetic work. BASELINE:root-{root}-child-{child}"),
                "fork_turns":"none"})).unwrap()}})).collect::<Vec<_>>();
        if !calls.is_empty() {
            let frame = json!({"choices":[{"delta":{"role":"assistant","tool_calls":calls},"finish_reason":null}]});
            let finish = json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]});
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {frame}\n\ndata: {finish}\n\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
            return;
        }
    }
    if matches!(case.work, Work::Stop) || mixed && wave % 5 == 4 {
        // The test cancels an actual live provider stream, never waits 30 seconds to acknowledge.
        if mixed {
            // Release the controlled provider's connection immediately when Core cancels it.
            // Keeping every cancelled socket asleep for 120s would measure fixture accumulation.
            let mut byte = [0u8; 1];
            let _ = tokio::time::timeout(Duration::from_secs(120), stream.read(&mut byte)).await;
        } else {
            tokio::time::sleep(Duration::from_secs(120)).await;
        }
        return;
    }
    if (needs_approval || matches!(case.work, Work::SlowTool)) && tool_results.is_empty() {
        let command = if matches!(case.work, Work::SlowTool) && owner == 0 {
            format!(
                "sleep {}; printf 'baseline-command-completed\\n'",
                case.slow_seconds
            )
        } else {
            "printf 'baseline-approved-once\\n'".into()
        };
        write_tool_call_stream(
            stream,
            "baseline-command",
            "run_command",
            json!({"command":command,
            "reason":"synthetic local performance fixture"}),
            "Preparing the synthetic command.",
        )
        .await;
        return;
    }
    if matches!(case.work, Work::SlowTool) {
        if let Some(result) = tool_results.last() {
            if result["status"] == "running" {
                if let Some(session) = result["sessionId"].as_str() {
                    write_tool_call_stream(
                        stream,
                        &format!("wait-{}", tool_results.len()),
                        "command_session",
                        json!({"sessionId":session,"action":"wait"}),
                        "",
                    )
                    .await;
                    return;
                }
            }
        }
    }
    // Fixed output size/rate independent of concurrency. Child work holds its provider briefly
    // so actual Dispatcher overlap is observable. This is model latency, not a fake DB handler.
    tokio::time::sleep(Duration::from_millis(50)).await;
    if mixed {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        // Same complete 243-byte answer as the baseline, delivered over 320ms to exercise ticks.
        let content = format!("Baseline complete. {}", "output ".repeat(32));
        for chunk in content.as_bytes().chunks(16) {
            let frame = json!({"choices":[{"delta":{"content":std::str::from_utf8(chunk).unwrap()},"finish_reason":null}]});
            stream
                .write_all(format!("data: {frame}\n\n").as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        stream.write_all(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").await.unwrap();
    } else {
        write_text_stream(
            stream,
            &format!("Baseline complete. {}", "output ".repeat(32)),
        )
        .await;
    }
}
async fn provider(
    case: Case,
    events: Arc<Mutex<Events>>,
    epoch: Instant,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
    Arc<AtomicUsize>,
) {
    // Keep the controlled model on one separate fixture thread. A stalled Core Tokio worker
    // must not accidentally slow the supposed 50 ms external provider and obscure attribution.
    let (listening, address) = std::sync::mpsc::sync_channel(1);
    let (stop, mut stopping) = tokio::sync::oneshot::channel();
    let (finished, finish) = tokio::sync::oneshot::channel();
    let peak = Arc::new(AtomicUsize::new(0));
    let peak_task = peak.clone();
    let provider_thread=std::thread::Builder::new().name("baseline-loopback-provider".into()).spawn(move || {
        let outcome=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async move {
                let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
                listening.send(format!("http://{}/v1/chat/completions",listener.local_addr().unwrap())).unwrap();
                let active=Arc::new(AtomicUsize::new(0));
                let mut clients=JoinSet::new();
                loop {
                    tokio::select! {
                        _=&mut stopping=>break,
                        Some(result)=clients.join_next(),if !clients.is_empty()=>{result.unwrap();},
                        incoming=listener.accept()=>{
                            let (mut stream,_)=incoming.unwrap();
                            let events=events.clone();let case=case.clone();let active=active.clone();let peak=peak_task.clone();
                            clients.spawn(async move {
                                let request=read_json_request(&mut stream).await;
                                sample(&events,|state| {state.provider_requests+=1;state.provider_request_bytes+=request.to_string().len();
                                    state.provider_first_us.entry(marker(&request)).or_insert_with(||us(epoch.elapsed()));});
                                let current=active.fetch_add(1,Ordering::SeqCst)+1;peak.fetch_max(current,Ordering::SeqCst);
                                provider_response(&mut stream,&request,&case).await;
                                active.fetch_sub(1,Ordering::SeqCst);
                            });
                        }
                    }
                }
                clients.abort_all();
                while let Some(result)=clients.join_next().await {if let Err(error)=result {assert!(error.is_cancelled(),"{error}");}}
            });
        }));
        let _=finished.send(outcome);
    }).unwrap();
    let url = address.recv().expect("local provider thread must bind");
    let task = tokio::spawn(async move {
        let outcome = finish.await.expect("provider completion channel");
        provider_thread
            .join()
            .expect("local provider thread joined");
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    });
    (url, stop, task, peak)
}
fn request(id: i64, method: &str, params: Value) -> JsonRpcRequest {
    serde_json::from_value(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        .unwrap()
}
// Test-only adapter keeps the production services and each scheduled request's identity explicit.
#[allow(clippy::too_many_arguments)]
fn submit(
    rpc: &BaselineDispatcher,
    storage: &Arc<StorageService>,
    service: &AgentService,
    outbound: &crate::transport::OutboundSender,
    events: &Arc<Mutex<Events>>,
    epoch: Instant,
    id: i64,
    owner: usize,
    phase: &'static str,
    scheduled_us: u64,
    method: &'static str,
    params: Value,
) {
    sample(events, |state| {
        state.rpcs.insert(
            id,
            RpcSample {
                id,
                owner,
                operation: method,
                phase,
                provider_key: params["content"].as_str().and_then(|text| {
                    text.rsplit_once("BASELINE:")
                        .map(|(_, tail)| tail.trim().to_string())
                }),
                scheduled_us,
                offered_us: us(epoch.elapsed()),
                handler_started_us: None,
                responded_us: None,
                dispatch_admitted: false,
                response: None,
                response_bytes: 0,
            },
        );
    });
    let handler_events = events.clone();
    let result = rpc.submit(
        storage.clone(),
        service.clone(),
        outbound.clone(),
        request(id, method, params),
        move || {
            sample(&handler_events, |state| {
                state.rpcs.get_mut(&id).unwrap().handler_started_us = Some(us(epoch.elapsed()))
            });
        },
    );
    sample(events, |state| {
        let record = state.rpcs.get_mut(&id).unwrap();
        match result {
            Ok(()) => record.dispatch_admitted = true,
            Err(response) => {
                record.responded_us = Some(us(epoch.elapsed()));
                record.response_bytes = response.to_string().len();
                record.response = Some(response);
            }
        }
    });
}
async fn until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    tokio::time::timeout(timeout, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}
fn root_input(unit: usize, wave: usize, case: &Case) -> Value {
    json!({"conversationId":format!("baseline-{unit}"),"projectId":"baseline-project","modelId":"model-1",
        "contextWindowIndicatorEnabled":true,"content":format!("Complete synthetic task. BASELINE:root-{unit}-wave-{wave}"),
        "attachments":[],"folderReferences":[],"skills":[],"userMessageId":format!("input-{unit}-{wave}"),
        "assistantMessageId":format!("output-{unit}-{wave}"),"permissions":AgentPermissions {
            command:if matches!(case.work, Work::Mixed) && wave % 5 == 3 || matches!(case.work,Work::Approval|Work::ApprovalBehindPreparation) || matches!(case.work,Work::SlowTool) && unit != 0 {mycopilot_core::AgentCommandPermission::RequireApproval}else{mycopilot_core::AgentCommandPermission::AutoApprove},
            write:AgentWritePermission::WorkspaceOnly,
            command_safety:mycopilot_core::AgentCommandSafetyPolicy::FullAccess,..AgentPermissions::default()}})
}

fn expected_load_rejection(operation: &str, response: &Value) -> bool {
    let error = &response["error"];
    (error["data"]["type"] == "rpc_dispatch_error" && error["data"]["code"] == "overloaded")
        || (operation == AGENT_START_CONVERSATION_TURN_METHOD
            && ((error["data"]["domain"] == "agent_turn"
                && error["data"]["code"] == "conversation_busy")
                || error["message"] == "Agent 全局并发上限为 50，请等待正在执行的 Turn 完成。"))
}

fn mixed_interval_ms(case: &Case) -> u64 {
    if matches!(case.work, Work::Mixed) {
        std::env::var("BACKEND_BASELINE_INTERVAL_MS")
            .ok()
            .map(|value| value.parse().expect("integer interval"))
            .unwrap_or(2_000)
    } else {
        2_000
    }
}

async fn run_case(case: Case) -> Value {
    let interval_ms = mixed_interval_ms(&case);
    assert!((250..=10_000).contains(&interval_ms));
    let fixture = tempdir().unwrap();
    let path = fixture.path().join("baseline.sqlite");
    let cold = Instant::now();
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let storage =
        Arc::new(StorageService::open_with_model_credentials(&path, credentials.clone()).unwrap());
    let database_open_us = us(cold.elapsed());
    storage
        .save_project(ProjectRecord::with_primary_folder(
            "baseline-project",
            "Synthetic performance fixture",
            fixture.path().to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    seed(&storage, case.roots, case.history_pairs);
    let epoch = Instant::now();
    let events = Arc::new(Mutex::new(Events::default()));
    let (url, stop_provider, provider_task, provider_peak) =
        provider(case.clone(), events.clone(), epoch).await;
    let mut settings = test_model_settings();
    settings.api_url = url;
    for model in &mut settings.models {
        model.context_window_tokens = Some(FIXTURE_CONTEXT_WINDOW_TOKENS);
    }
    storage.save_model_settings(settings).unwrap();
    let fixture_context_window_tokens = storage
        .load_model_settings()
        .unwrap()
        .unwrap()
        .models
        .iter()
        .find(|model| model.id == "model-1")
        .unwrap()
        .effective_context_window_tokens();
    assert_eq!(fixture_context_window_tokens, FIXTURE_CONTEXT_WINDOW_TOKENS);
    let startup = Instant::now();
    let service = AgentService::new(storage.clone());
    // Match the Host's finite lease renewal, without extending the production 60s TTL or
    // bypassing admission checks. The fixture renews only for its bounded planned lifetime.
    let execution_access_coverage_ms = case.waves.saturating_sub(1) as u64 * interval_ms
        + case.slow_seconds * 1_000
        + 25
        + 300_000;
    let authorization_started = tokio::time::Instant::now();
    let authorization_deadline =
        authorization_started + Duration::from_millis(execution_access_coverage_ms);
    let grant = |service: &AgentService, revision: u64, ttl_ms: i64| {
        let issued_at = now_ms();
        let output = service
            .set_execution_access(SetExecutionAccessInput {
                revision,
                identity_epoch: 0,
                reason: ExecutionAccessReason::Allowed,
                issued_at,
                valid_until: issued_at + ttl_ms,
            })
            .expect("baseline execution lease uses the production validation path");
        assert_eq!(
            output.revision, revision,
            "lease refresh must not be ignored as stale"
        );
        service
            .check_automation_execution_access()
            .expect("accepted baseline lease must authorize the existing admission path");
    };
    grant(&service, 1, 60_000);
    sample(&events, |state| {
        state.execution_access_grants.push(
            json!({"at_us":us(epoch.elapsed()),"revision":1,"identity_epoch":0,"ttl_ms":60_000}),
        )
    });
    let (stop_authorization, mut authorization_stopped) = tokio::sync::oneshot::channel();
    let authorization_service = service.clone();
    let authorization_events = events.clone();
    let authorization_task = tokio::spawn(async move {
        let mut revision = 1;
        let mut next = authorization_started + Duration::from_secs(20);
        loop {
            tokio::select! {
                _ = &mut authorization_stopped => break,
                _ = tokio::time::sleep_until(next.min(authorization_deadline)) => {
                    let remaining = authorization_deadline.saturating_duration_since(tokio::time::Instant::now()).as_millis();
                    if remaining == 0 { break; }
                    revision += 1;
                    let ttl_ms = remaining.min(60_000) as i64;
                    grant(&authorization_service, revision, ttl_ms);
                    sample(&authorization_events, |state| state.execution_access_grants.push(json!({
                        "at_us":us(epoch.elapsed()),"revision":revision,"identity_epoch":0,"ttl_ms":ttl_ms})));
                    next = tokio::time::Instant::now() + Duration::from_secs(20);
                }
            }
        }
    });
    let service_startup_us = us(startup.elapsed());
    assert_eq!(
        service.turn_concurrency_gate().limit(),
        50,
        "baseline must retain production capacity"
    );
    let (outbound, mut receiver) = crate::transport::outbound_channel();
    let rpc = BaselineDispatcher::new(outbound.clone());
    let collector_events = events.clone();
    let collector = tokio::spawn(async move {
        while let Some(value) = receiver.recv().await {
            sample(&collector_events, |state| {
                if let Some(id) = value["id"].as_i64() {
                    if let Some(record) = state.rpcs.get_mut(&id) {
                        record.responded_us = Some(us(epoch.elapsed()));
                        record.response_bytes = value.to_string().len();
                        // Bound the harness itself: never retain full growing history responses.
                        record.response = Some(
                            if record.operation == "storage.loadConversation"
                                && value["result"].is_object()
                            {
                                json!({"id":id,"result":{"observed":true,"id":value["result"]["id"],"messages":value["result"]["messages"].as_array().map(Vec::len)}})
                            } else {
                                value
                            },
                        );
                    }
                    if let Some(record) = state.rpcs.get(&id) {
                        if record.operation == AGENT_START_CONVERSATION_TURN_METHOD {
                            if let Some(key) = record.provider_key.as_ref().filter(|key| {
                                key.split("-wave-")
                                    .nth(1)
                                    .and_then(|wave| wave.parse::<usize>().ok())
                                    .is_some_and(|wave| wave % 5 == 4)
                            }) {
                                if let Some(run) = record
                                    .response
                                    .as_ref()
                                    .and_then(|response| response["result"]["runId"].as_str())
                                {
                                    state.ready_stops.push((
                                        record.owner,
                                        run.to_owned(),
                                        key.clone(),
                                    ));
                                }
                            }
                        }
                    }
                    return;
                }
                state.output_events += 1;
                state.output_bytes += value.to_string().len();
                let event = if value["method"] == "agent.collaboration.event" {
                    &value["params"]["event"]
                } else {
                    &value["params"]
                };
                if let Some(run) = event["runId"].as_str() {
                    if event["type"] == "command_started" {
                        state
                            .command_started_us
                            .entry(run.into())
                            .or_insert_with(|| us(epoch.elapsed()));
                    } else if event["type"] == "command_exited" {
                        state
                            .command_exited_us
                            .entry(run.into())
                            .or_insert_with(|| us(epoch.elapsed()));
                    }
                    if event["type"] == "context_window_updated" {
                        let snapshot = &event["snapshot"];
                        let peak = state
                            .context_peak_by_run
                            .entry(run.into())
                            .or_insert_with(|| snapshot.clone());
                        if snapshot["inputTokens"].as_u64().unwrap_or(0)
                            > peak["inputTokens"].as_u64().unwrap_or(0)
                        {
                            *peak = snapshot.clone();
                        }
                    }
                }
                if matches!(
                    event["type"].as_str(),
                    Some("context_compaction_started" | "context_compaction_finished")
                ) {
                    state.context_compaction_events += 1;
                }
                if event["type"] == "done"
                    && matches!(
                        event["status"].as_str(),
                        Some("completed" | "failed" | "cancelled")
                    )
                {
                    if let Some(run) = event["runId"].as_str() {
                        state.terminal.insert(
                            run.into(),
                            json!({"at_us":us(epoch.elapsed()),"status":event["status"]}),
                        );
                    }
                }
                if event["type"] == "error" {
                    state.errors.push(event.clone());
                }
            });
        }
    });
    let timer_stop = Arc::new(AtomicBool::new(false));
    let timer_flag = timer_stop.clone();
    let timer_events = events.clone();
    let timer = tokio::spawn(async move {
        let mut next = tokio::time::Instant::now() + Duration::from_millis(20);
        while !timer_flag.load(Ordering::Relaxed) {
            tokio::time::sleep_until(next).await;
            sample(&timer_events, |state| {
                state.timer_lag_us.push(us(
                    tokio::time::Instant::now().saturating_duration_since(next)
                ))
            });
            // Preserve absolute deadlines; do not silently reset the clock after a stall.
            next += Duration::from_millis(20);
        }
    });
    let _ = mycopilot_core::performance::take_snapshot();
    let _ = mycopilot_core::performance::take_queue_snapshot();
    let before = usage();
    let started = Instant::now();
    let start_at = tokio::time::Instant::now() + Duration::from_millis(25);
    let scheduled_epoch = us(epoch.elapsed()) + 25_000;
    let mut next_id = 1_i64;
    let mut controlled = HashSet::<String>::new();
    let mut next_read = scheduled_epoch;
    let mut next_resource = scheduled_epoch;
    let mut read_owner = 0;
    let mut drive_mixed = |next_id: &mut i64| {
        if !matches!(case.work, Work::Mixed) {
            return;
        }
        let now = us(epoch.elapsed());
        // Read frequency is fixed and independent of response/completion; overload is retained.
        while now >= next_read
            && next_read
                <= scheduled_epoch + case.waves.saturating_sub(1) as u64 * interval_ms * 1_000
        {
            submit(
                &rpc,
                &storage,
                &service,
                &outbound,
                &events,
                epoch,
                *next_id,
                read_owner,
                "background_query",
                next_read,
                "storage.loadConversation",
                json!({"conversationId":format!("baseline-{read_owner}")}),
            );
            *next_id += 1;
            read_owner = (read_owner + 1) % case.roots;
            next_read += 500_000;
        }
        for action in service.list_pending_actions() {
            if controlled.insert(action.run_id.clone()) {
                let owner = events
                    .lock()
                    .unwrap()
                    .rpcs
                    .values()
                    .find(|record| {
                        record
                            .response
                            .as_ref()
                            .is_some_and(|response| response["result"]["runId"] == action.run_id)
                    })
                    .map(|record| record.owner)
                    .unwrap_or(0);
                submit(
                    &rpc,
                    &storage,
                    &service,
                    &outbound,
                    &events,
                    epoch,
                    *next_id,
                    owner,
                    "mixed_approval",
                    now,
                    AGENT_APPROVE_ACTION_METHOD,
                    json!({"runId":action.run_id,"actionId":action.action_id,"approvalScope":"singleAction"}),
                );
                *next_id += 1;
            }
        }
        let stops = {
            let mut state = events.lock().unwrap();
            let pending = std::mem::take(&mut state.ready_stops);
            let mut ready = Vec::new();
            for (owner, run, key) in pending {
                if state.provider_first_us.contains_key(&key) {
                    ready.push((owner, run));
                } else {
                    state.ready_stops.push((owner, run, key));
                }
            }
            ready
        };
        for (owner, run) in stops {
            if controlled.insert(run.clone()) {
                submit(
                    &rpc,
                    &storage,
                    &service,
                    &outbound,
                    &events,
                    epoch,
                    *next_id,
                    owner,
                    "mixed_stop",
                    now,
                    AGENT_CANCEL_RUN_METHOD,
                    json!({"runId":run}),
                );
                *next_id += 1;
            }
        }
        if now >= next_resource {
            sample(&events, |state| {
                state.resource_samples.push(json!({"at_us":now,
                "active_turns":service.turn_concurrency_gate().active(),"disk":disk_files(&path),
                "usage":usage(),"queues":mycopilot_core::performance::take_queue_snapshot()}))
            });
            next_resource += 1_000_000;
        }
    };
    for wave in 0..case.waves {
        // Open-loop offers: no response or previous run is awaited before the next scheduled wave.
        let deadline = start_at + Duration::from_millis(wave as u64 * interval_ms);
        if matches!(case.work, Work::Mixed) {
            while tokio::time::Instant::now() < deadline {
                drive_mixed(&mut next_id);
                tokio::time::sleep_until(
                    deadline.min(tokio::time::Instant::now() + Duration::from_millis(20)),
                )
                .await;
            }
            drive_mixed(&mut next_id);
        } else {
            tokio::time::sleep_until(deadline).await;
        }
        let phase = if wave == 0 {
            "cold_history"
        } else {
            "warm_history"
        };
        for owner in 0..if matches!(case.work, Work::ApprovalBehindPreparation) {
            1
        } else {
            case.roots
        } {
            submit(
                &rpc,
                &storage,
                &service,
                &outbound,
                &events,
                epoch,
                next_id,
                owner,
                phase,
                scheduled_epoch + wave as u64 * interval_ms * 1_000,
                AGENT_START_CONVERSATION_TURN_METHOD,
                root_input(owner, wave, &case),
            );
            next_id += 1;
        }
    }
    if matches!(case.work, Work::Mixed) {
        let drain = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            drive_mixed(&mut next_id);
            let awaiting = {
                let state = events.lock().unwrap();
                state.rpcs.values().any(|record| record.response.is_none())
            };
            if !awaiting && service.turn_concurrency_gate().active() == 0 {
                break;
            }
            if tokio::time::Instant::now() >= drain {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let starts_responded = until(Duration::from_secs(60), || {
        events
            .lock()
            .unwrap()
            .rpcs
            .values()
            .all(|r| r.response.is_some())
    })
    .await;
    let mut starts = events
        .lock()
        .unwrap()
        .rpcs
        .values()
        .filter_map(|r| {
            r.response
                .as_ref()
                .and_then(|v| v["result"]["runId"].as_str())
                .map(|run| (r.owner, run.to_string()))
        })
        .collect::<Vec<_>>();
    let mut all_controls_ready = true;
    let slow_run = matches!(case.work, Work::SlowTool)
        .then(|| {
            starts
                .iter()
                .find(|(owner, _)| *owner == 0)
                .map(|(_, run)| run.clone())
        })
        .flatten();
    if matches!(
        case.work,
        Work::Approval | Work::ApprovalBehindPreparation | Work::SlowTool
    ) {
        let expected = starts
            .len()
            .saturating_sub(usize::from(matches!(case.work, Work::SlowTool)));
        all_controls_ready = until(Duration::from_secs(30), || {
            service.list_pending_actions().len() >= expected
        })
        .await;
        if matches!(case.work, Work::SlowTool) {
            // Pending approvals alone do not prove the slow command has actually started.
            // Gate on its durable command_started notification before offering any approvals.
            all_controls_ready &= until(Duration::from_secs(30), || {
                slow_run
                    .as_ref()
                    .is_some_and(|run| events.lock().unwrap().command_started_us.contains_key(run))
            })
            .await;
        }
        let pending = service.list_pending_actions();
        if matches!(case.work, Work::ApprovalBehindPreparation) {
            let scheduled = us(epoch.elapsed());
            for owner in 1..case.roots {
                submit(
                    &rpc,
                    &storage,
                    &service,
                    &outbound,
                    &events,
                    epoch,
                    next_id,
                    owner,
                    "background_preparation",
                    scheduled,
                    AGENT_START_CONVERSATION_TURN_METHOD,
                    root_input(owner, 0, &case),
                );
                next_id += 1;
            }
        }
        let scheduled = us(epoch.elapsed());
        for (index, action) in pending.iter().enumerate() {
            let owner = starts
                .iter()
                .find(|(_, run)| run == &action.run_id)
                .map(|(owner, _)| *owner)
                .unwrap_or(index);
            submit(
                &rpc,
                &storage,
                &service,
                &outbound,
                &events,
                epoch,
                next_id,
                owner,
                "approval_burst",
                scheduled,
                AGENT_APPROVE_ACTION_METHOD,
                json!({"runId":action.run_id,"actionId":action.action_id,"approvalScope":"singleAction"}),
            );
            next_id += 1;
        }
    } else if matches!(case.work, Work::Stop) {
        all_controls_ready = until(Duration::from_secs(30), || {
            events.lock().unwrap().provider_first_us.len() >= starts.len()
        })
        .await;
        let scheduled = us(epoch.elapsed());
        for (owner, run) in &starts {
            submit(
                &rpc,
                &storage,
                &service,
                &outbound,
                &events,
                epoch,
                next_id,
                *owner,
                "stop_burst",
                scheduled,
                AGENT_CANCEL_RUN_METHOD,
                json!({"runId":run}),
            );
            next_id += 1;
        }
    }
    let replies_complete = until(Duration::from_secs(60), || {
        events
            .lock()
            .unwrap()
            .rpcs
            .values()
            .all(|r| r.response.is_some())
    })
    .await;
    starts = events
        .lock()
        .unwrap()
        .rpcs
        .values()
        .filter(|r| r.operation == AGENT_START_CONVERSATION_TURN_METHOD)
        .filter_map(|r| {
            r.response
                .as_ref()
                .and_then(|v| v["result"]["runId"].as_str())
                .map(|run| (r.owner, run.to_string()))
        })
        .collect();
    let tree_probe = matches!(case.work, Work::Tree).then(|| {
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
    });
    // If bounded approval/control admission rejects a burst, preserve the rejection in raw data
    // and perform a separately labelled cleanup later; never pretend every run completed.
    let has_rejected_control = events.lock().unwrap().rpcs.values().any(|r| {
        r.operation != AGENT_START_CONVERSATION_TURN_METHOD
            && r.operation != "storage.loadConversation"
            && r.response
                .as_ref()
                .is_some_and(|v| v.get("error").is_some())
    });
    let timeout = if has_rejected_control {
        Duration::from_secs(2)
    } else {
        Duration::from_secs(45 + case.slow_seconds)
    };
    let drained = until(timeout, || {
        tree_probe.as_ref().is_none_or(|connection|connection.query_row(
            "SELECT COUNT(*) FROM agent_wake_requests WHERE status IN ('queued','claimed','running','waiting_for_approval')", [],
            |row|row.get::<_,i64>(0)).unwrap() == 0)
        && service.turn_concurrency_gate().active() == 0
            && (!matches!(case.work, Work::Tree) || {
                (0..case.roots)
                    .map(|root| {
                        storage
                            .get_agent_node_by_conversation(&format!("baseline-{root}"))
                            .unwrap()
                            .map(|node| storage.list_agent_tree(&node.root_agent_id).unwrap().len())
                            .unwrap_or(0)
                    })
                    .sum::<usize>()
                    == case.units
            })
            && storage
                .list_in_progress_conversation_turn_traces()
                .unwrap()
                .is_empty()
    })
    .await;
    drop(tree_probe);
    let _ = stop_authorization.send(());
    authorization_task.await.unwrap();
    let execution_access_actual_ms = authorization_started.elapsed().as_millis() as u64;
    let measured_us = us(started.elapsed());
    let perf = mycopilot_core::performance::take_snapshot();
    let queue_metrics = mycopilot_core::performance::take_queue_snapshot();
    let after = usage();
    timer_stop.store(true, Ordering::Relaxed);
    timer.await.unwrap();
    let outbound_stats = outbound.stats();
    let remaining_before_cleanup = storage
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .len();
    rpc.shutdown().await;
    // Stop only this fixture's processes and dispatcher. User sessions are never inspected.
    for (_, run) in &starts {
        if !drained {
            let _ = service.cancel_run_checked(run);
        }
    }
    let (_, shutdown_timed_out) = service.shutdown_active_runs(Duration::from_secs(10)).await;
    service.shutdown_collaboration_dispatcher().await.unwrap();
    let _ = stop_provider.send(());
    provider_task.await.unwrap();
    collector.abort();
    let _ = collector.await;
    let sqlite = sqlite_info(&path);
    let disk = disk_files(&path);
    let before_reopen = storage
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .len();
    drop(service);
    drop(storage);
    drop(outbound);
    let recovery_started = Instant::now();
    let reopened =
        Arc::new(StorageService::open_with_model_credentials(&path, credentials).unwrap());
    let recovered_service = AgentService::new_authorized_for_test(reopened.clone());
    let reopen_us = us(recovery_started.elapsed());
    let in_progress_after_reopen = reopened
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .len();
    let mut durable_terminals = BTreeMap::<String, usize>::new();
    let mut recovered_root_turns = 0;
    let mut child_terminals = BTreeMap::<String, usize>::new();
    let mut recovered_child_turns = 0;
    let mut child_recovery = BTreeMap::<String, Value>::new();
    for owner in 0..case.roots {
        for trace in reopened
            .list_conversation_turn_traces(&format!("baseline-{owner}"))
            .unwrap()
        {
            if trace.assistant_message_id.starts_with("output-") {
                *durable_terminals
                    .entry(format!("{:?}", trace.terminal_status))
                    .or_default() += 1;
                recovered_root_turns += 1;
            }
        }
        if matches!(case.work, Work::Tree) {
            if let Some(root) = reopened
                .get_agent_node_by_conversation(&format!("baseline-{owner}"))
                .unwrap()
            {
                for child in reopened
                    .list_agent_tree(&root.root_agent_id)
                    .unwrap()
                    .into_iter()
                    .filter(|node| node.agent_id != root.agent_id)
                {
                    let traces = reopened
                        .list_conversation_turn_traces(&child.conversation_id)
                        .unwrap();
                    let statuses = traces
                        .iter()
                        .map(|trace| format!("{:?}", trace.terminal_status))
                        .collect::<Vec<_>>();
                    for status in &statuses {
                        *child_terminals.entry(status.clone()).or_default() += 1;
                        recovered_child_turns += 1;
                    }
                    child_recovery.insert(
                        child.conversation_id,
                        json!({
                        "agent_id":child.agent_id,"terminal_statuses":statuses,
                        "one_completed_turn":statuses.len()==1 && statuses[0]=="Completed"}),
                    );
                }
            }
        }
    }
    recovered_service
        .shutdown_active_runs(Duration::from_secs(5))
        .await;
    recovered_service
        .shutdown_collaboration_dispatcher()
        .await
        .unwrap();
    drop(recovered_service);
    drop(reopened);
    let state = events.lock().unwrap();
    let slow_tool_overlap = if matches!(case.work, Work::SlowTool) {
        let command_started = slow_run
            .as_ref()
            .and_then(|run| state.command_started_us.get(run))
            .copied();
        let command_exited = slow_run
            .as_ref()
            .and_then(|run| state.command_exited_us.get(run))
            .copied();
        let approval_offered = state
            .rpcs
            .values()
            .filter(|record| record.operation == AGENT_APPROVE_ACTION_METHOD)
            .map(|record| record.offered_us)
            .collect::<Vec<_>>();
        let observed_overlap = !approval_offered.is_empty()
            && command_started
                .zip(command_exited)
                .is_some_and(|(start, end)| {
                    approval_offered
                        .iter()
                        .all(|offered| *offered >= start && *offered <= end)
                });
        json!({"slow_run_id":slow_run,"command_started_us":command_started,"command_exited_us":command_exited,
            "approval_offered_us":approval_offered,"all_approvals_offered_during_command":observed_overlap,
            "boundary":"Core outbound receipt of durable command lifecycle events and RPC offer; same monotonic clock"})
    } else {
        Value::Null
    };
    let mut metrics = serde_json::Map::new();
    for operation in [
        AGENT_START_CONVERSATION_TURN_METHOD,
        AGENT_APPROVE_ACTION_METHOD,
        AGENT_CANCEL_RUN_METHOD,
        "storage.loadConversation",
    ] {
        let records = state
            .rpcs
            .values()
            .filter(|r| r.operation == operation)
            .collect::<Vec<_>>();
        if records.is_empty() {
            continue;
        }
        metrics.insert(operation.into(),json!({
            "offered":records.len(),"dispatch_admitted":records.iter().filter(|r|r.dispatch_admitted).count(),
            "success_response":records.iter().filter(|r|r.response.as_ref().is_some_and(|v|v.get("result").is_some())).count(),
            "rejected_or_error":records.iter().filter(|r|r.response.as_ref().is_some_and(|v|v.get("error").is_some())).count(),
            "no_response":records.iter().filter(|r|r.response.is_none()).count(),
            "busy_or_capacity_rejections":records.iter().filter(|r|r.response.as_ref().is_some_and(|v|expected_load_rejection(r.operation,v))).count(),
            "intake_schedule_lag":quantiles(records.iter().map(|r|r.offered_us.saturating_sub(r.scheduled_us)).collect()),
            "queue":quantiles(records.iter().filter_map(|r|r.handler_started_us.map(|v|v.saturating_sub(r.offered_us))).collect()),
            "handler_to_response":quantiles(records.iter().filter_map(|r|r.handler_started_us.zip(r.responded_us).map(|(a,b)|b.saturating_sub(a))).collect()),
            "scheduled_to_response":quantiles(records.iter().filter_map(|r|r.responded_us.map(|v|v.saturating_sub(r.scheduled_us))).collect()),
            "actual_offer_to_response":quantiles(records.iter().filter_map(|r|r.responded_us.map(|v|v.saturating_sub(r.offered_us))).collect()),
            "scheduled_to_provider_first_request":quantiles(records.iter().filter_map(|r|r.provider_key.as_ref()
                .and_then(|key|state.provider_first_us.get(key)).map(|at|at.saturating_sub(r.scheduled_us))).collect())}));
    }
    let mut fairness = BTreeMap::<usize, Value>::new();
    for owner in 0..case.roots {
        let records = state
            .rpcs
            .values()
            .filter(|r| r.owner == owner)
            .collect::<Vec<_>>();
        fairness.insert(owner,json!({"offered":records.len(),"responses":records.iter().filter(|r|r.response.is_some()).count(),
            "scheduled_to_response":quantiles(records.iter().filter_map(|r|r.responded_us.map(|v|v.saturating_sub(r.scheduled_us))).collect())}));
    }
    let mut report = json!({"kind":"backend_concurrency_case","format_version":2,"case":case,"name":case.name(),
        "boundary":"scheduled Core intake -> production bounded dispatcher -> real handler -> AgentService -> SQLite/local provider/tools -> outbound receipt; no Electron/renderer",
        "build_profile":if cfg!(debug_assertions){"debug"}else{"release"},
        "runtime_workers":std::thread::available_parallelism().map(|n|n.get()).unwrap_or(1),
        "fixture_provider_threads":1,"logical_turn_limit":50,"fixed_wave_interval_ms":interval_ms,
        "mixed_workload":if matches!(case.work,Work::Mixed) {json!({"stream_chunk_bytes":16,"stream_interval_ms":20,"query_interval_ms":500,"cycle":["text","text","text","approval","stop"]})} else {Value::Null},
        "fixture_context_window_tokens":fixture_context_window_tokens,
        "execution_access_fixture":{"lease_ttl_ms":60_000,"refresh_interval_ms":20_000,
            "planned_coverage_ms":execution_access_coverage_ms,"actual_renewal_task_ms":execution_access_actual_ms,
            "refresh_count":state.execution_access_grants.len().saturating_sub(1)},
        "provider_delay_ms":50,"provider_output_bytes":"Baseline complete. ".len()+"output ".len()*32,
        "database_open_us":database_open_us,"service_startup_us":service_startup_us,"sqlite":sqlite});
    let lifecycle = json!({"measured_us":measured_us,"root_starts_durably_accepted":starts.len(),
        "starts_responded":starts_responded,"controls_ready":all_controls_ready,"replies_complete":replies_complete,
        "drained_before_cleanup":drained,"in_progress_before_cleanup":remaining_before_cleanup,
        "shutdown_timed_out":shutdown_timed_out,"control_rejections_observed":has_rejected_control,
        "recovered_root_turns":recovered_root_turns,"recovered_child_turns":recovered_child_turns,
        "slow_tool_overlap":slow_tool_overlap,
        "recovery":{"open_service_us":reopen_us,"pre_close_in_progress":before_reopen,
            "post_reopen_in_progress":in_progress_after_reopen,"root_terminal_statuses":durable_terminals,
            "child_terminal_statuses":child_terminals,"children":child_recovery,
            "kind":"clean shutdown/reopen; not a crash or power-loss test"},
        "completed_root_turns_per_second":durable_terminals.get("Completed").copied().unwrap_or(0) as f64/(measured_us as f64/1_000_000.0)});
    report
        .as_object_mut()
        .unwrap()
        .extend(lifecycle.as_object().unwrap().clone());
    let unexpected = state
        .rpcs
        .values()
        .filter_map(|record| {
            record
                .response
                .as_ref()
                .map(|response| (record.operation, response))
        })
        .filter(|(operation, response)| {
            response.get("error").is_some() && !expected_load_rejection(operation, response)
        })
        .map(|(_, response)| response)
        .collect::<Vec<_>>();
    let measurements = json!({"metrics":metrics,"fairness_by_synthetic_owner":fairness,"stage_metrics":perf,
        "queue_metrics":queue_metrics,"queue_metrics_scope":if matches!(case.work, Work::Mixed) {"final_window; aggregate all raw.resource_samples windows for case peaks"} else {"whole_case"},"unexpected_rpc_errors":unexpected,
        "timer_lag":quantiles(state.timer_lag_us.clone()),"resources_before":before,"resources_after":after,
        "disk_after":disk,"provider_peak_connections":provider_peak.load(Ordering::SeqCst),
        "outbound":{"peak_bytes":outbound_stats.peak_bytes,"peak_frames":outbound_stats.peak_frames,
            "serialized_bytes":outbound_stats.serialized_bytes,"merged_events":outbound_stats.merged_events},"raw":*state});
    report
        .as_object_mut()
        .unwrap()
        .extend(measurements.as_object().unwrap().clone());
    report
}

#[test]
#[ignore = "explicit local disk/loopback performance experiment; run with BACKEND_BASELINE_OUTPUT"]
fn backend_concurrency_disk_baseline() {
    let output =
        std::env::var("BACKEND_BASELINE_OUTPUT").expect("set an explicit JSONL output path");
    assert_eq!(
        std::env::var("CAPTAIN_PERFORMANCE_DIAGNOSTICS").as_deref(),
        Ok("1"),
        "stage metrics must be enabled before process startup"
    );
    let smoke = std::env::var("BACKEND_BASELINE_MODE").as_deref() == Ok("smoke");
    let filter = std::env::var("BACKEND_BASELINE_FILTER").ok();
    let waves = std::env::var("BACKEND_BASELINE_WAVES")
        .ok()
        .map(|value| value.parse::<usize>().expect("integer waves"))
        .unwrap_or(if smoke { 1 } else { 3 });
    assert!(
        (1..=900).contains(&waves),
        "at most 30 minutes per text case"
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(
            std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(1),
        )
        .enable_all()
        .build()
        .unwrap();
    let mut cases = Vec::new();
    for units in if smoke {
        vec![1, 5]
    } else {
        vec![1, 5, 10, 20, 50]
    } {
        for history_pairs in if smoke { vec![0] } else { vec![0, 128] } {
            cases.push(Case {
                units,
                roots: units,
                history_pairs,
                work: Work::Text,
                slow_seconds: 0,
                waves,
            });
        }
        cases.push(Case {
            units,
            roots: units,
            history_pairs: 0,
            work: Work::Approval,
            slow_seconds: 0,
            waves: 1,
        });
        if units >= 5 {
            cases.push(Case {
                units,
                roots: units,
                history_pairs: if smoke { 8 } else { 128 },
                work: Work::ApprovalBehindPreparation,
                slow_seconds: 0,
                waves: 1,
            });
        }
        cases.push(Case {
            units,
            roots: units,
            history_pairs: 0,
            work: Work::Stop,
            slow_seconds: 0,
            waves: 1,
        });
        if units > 1 {
            cases.push(Case {
                units,
                roots: 1,
                history_pairs: 0,
                work: Work::Tree,
                slow_seconds: 0,
                waves: 1,
            });
            if units >= 5 {
                cases.push(Case {
                    units,
                    roots: 2,
                    history_pairs: 0,
                    work: Work::Tree,
                    slow_seconds: 0,
                    waves: 1,
                });
            }
        }
    }
    for slow_seconds in if smoke { vec![1] } else { vec![1, 10, 30] } {
        // Stay within the existing 32-command Session limit; the 50-run capacity is tested above.
        cases.push(Case {
            units: 5,
            roots: 5,
            history_pairs: 0,
            work: Work::SlowTool,
            slow_seconds,
            waves: 1,
        });
    }
    if !smoke {
        // Fixed open-loop schedule: 20 root turns offered every 2 seconds for 60 seconds,
        // regardless of previous completion. This is a baseline, not the final soak gate.
        cases.push(Case {
            units: 20,
            roots: 20,
            history_pairs: 0,
            work: Work::SteadyText,
            slow_seconds: 0,
            waves: 31,
        });
    }
    if std::env::var("BACKEND_BASELINE_MODE").as_deref() == Ok("soak") {
        let units = std::env::var("BACKEND_BASELINE_UNITS")
            .ok()
            .map(|value| value.parse::<usize>().expect("integer units"))
            .unwrap_or(20);
        assert!([1, 5, 10, 20, 50].contains(&units));
        let interval = std::env::var("BACKEND_BASELINE_INTERVAL_MS")
            .ok()
            .map(|value| value.parse::<u64>().expect("integer interval"))
            .unwrap_or(2_000);
        let duration = std::env::var("BACKEND_BASELINE_DURATION_SECONDS")
            .ok()
            .map(|value| value.parse::<u64>().expect("integer duration"))
            .unwrap_or(600);
        assert!((10..=1_800).contains(&duration));
        assert!((250..=10_000).contains(&interval));
        cases = vec![Case {
            units,
            roots: units,
            history_pairs: 0,
            work: Work::Mixed,
            slow_seconds: 0,
            waves: (duration * 1000 / interval + 1) as usize,
        }];
    }
    let selected = cases
        .into_iter()
        .filter(|case| {
            filter
                .as_ref()
                .is_none_or(|filter| case.name().contains(filter))
        })
        .collect::<Vec<_>>();
    assert!(
        !selected.is_empty(),
        "BACKEND_BASELINE_FILTER matched no cases"
    );
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output)
        .expect("use a new result path; never overwrite an earlier baseline");
    let mut failures = Vec::new();
    for case in selected {
        let result = runtime.block_on(run_case(case));
        writeln!(file, "{result}").unwrap();
        file.flush().unwrap();
        eprintln!("backend baseline {}: {}", result["name"], result["metrics"]);
        if result["root_starts_durably_accepted"].as_u64().unwrap_or(0)
            < result["case"]["roots"].as_u64().unwrap()
            || (result["drained_before_cleanup"] != true
                && result["control_rejections_observed"] != true)
            || result["starts_responded"] != true
            || result["replies_complete"] != true
            || result["controls_ready"] != true
            || result["shutdown_timed_out"] != false
            || result["recovery"]["post_reopen_in_progress"] != 0
            || result["unexpected_rpc_errors"]
                .as_array()
                .is_some_and(|errors| !errors.is_empty())
            || result["raw"]["errors"]
                .as_array()
                .is_some_and(|errors| !errors.is_empty())
            || result["raw"]["context_compaction_events"] != 0
            || result["raw"]["provider_first_us"].get("unknown").is_some()
            || result["raw"]["context_peak_by_run"]
                .as_object()
                .unwrap()
                .values()
                .any(|snapshot| snapshot["contextWindowTokens"] != FIXTURE_CONTEXT_WINDOW_TOKENS)
            || (result["case"]["work"] == "tree"
                && (result["recovered_child_turns"].as_u64().unwrap_or(0)
                    != result["case"]["units"].as_u64().unwrap()
                        - result["case"]["roots"].as_u64().unwrap()
                    || result["recovery"]["child_terminal_statuses"]["Completed"]
                        .as_u64()
                        .unwrap_or(0)
                        != result["case"]["units"].as_u64().unwrap()
                            - result["case"]["roots"].as_u64().unwrap()
                    || result["recovery"]["children"].as_object().unwrap().len() as u64
                        != result["case"]["units"].as_u64().unwrap()
                            - result["case"]["roots"].as_u64().unwrap()
                    || result["recovery"]["children"]
                        .as_object()
                        .unwrap()
                        .values()
                        .any(|child| child["one_completed_turn"] != true)))
            || (result["case"]["work"] == "slow_tool"
                && result["slow_tool_overlap"]["all_approvals_offered_during_command"] != true)
        {
            failures.push(result["name"].clone());
        }
    }
    file.sync_all().unwrap();
    assert!(
        failures.is_empty(),
        "baseline correctness/fixture failures (raw results preserved): {failures:?}"
    );
}

#[test]
fn backend_baseline_quantiles_are_exact_and_small_p99_is_labelled() {
    let report = quantiles((1..=100).collect());
    assert_eq!(report["p50_us"], 50);
    assert_eq!(report["p95_us"], 95);
    assert_eq!(report["p99_us"], 99);
    assert_eq!(report["p99_reliable"], false);
    assert_eq!(quantiles(vec![])["p95_us"], Value::Null);
}

#[test]
fn backend_baseline_payloads_use_current_protocol_contracts() {
    for work in [
        Work::Text,
        Work::SteadyText,
        Work::Mixed,
        Work::Approval,
        Work::ApprovalBehindPreparation,
        Work::Stop,
        Work::SlowTool,
        Work::Tree,
    ] {
        let case = Case {
            units: 5,
            roots: 5,
            history_pairs: 0,
            work,
            slow_seconds: 1,
            waves: 1,
        };
        let input: AgentConversationTurnInput =
            serde_json::from_value(root_input(0, 0, &case)).unwrap();
        assert_eq!(input.model_id, "model-1");
    }
    let _: mycopilot_protocol_rs::AgentApproveActionRequest = serde_json::from_value(json!({
        "runId":"synthetic-run","actionId":"synthetic-action","approvalScope":"singleAction"}))
    .unwrap();
}

#[test]
fn backend_baseline_only_classifies_explicit_load_rejections() {
    let busy =
        json!({"error":{"code":-32000,"data":{"domain":"agent_turn","code":"conversation_busy"}}});
    assert!(expected_load_rejection(
        AGENT_START_CONVERSATION_TURN_METHOD,
        &busy
    ));
    assert!(!expected_load_rejection(AGENT_APPROVE_ACTION_METHOD, &busy));
    assert!(!expected_load_rejection(
        AGENT_APPROVE_ACTION_METHOD,
        &json!({"error":{"code":-32602,"message":"missing approvalScope"}})
    ));
    assert!(!expected_load_rejection(
        AGENT_START_CONVERSATION_TURN_METHOD,
        &json!({"error":{"code":-32000,"message":"database error"}})
    ));
}

#[test]
fn mixed_provider_only_consumes_tool_results_from_the_current_turn() {
    let mut request = json!({"messages":[
        {"role":"user","content":"BASELINE:root-0-wave-3"},
        {"role":"tool","content":"{\"status\":\"completed\"}"},
        {"role":"user","content":"BASELINE:root-0-wave-8"}
    ]});
    assert!(current_tool_results(&request, true).is_empty());
    assert_eq!(current_tool_results(&request, false).len(), 1);
    request["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"tool","content":"{\"status\":\"completed\"}"}));
    assert_eq!(current_tool_results(&request, true).len(), 1);
}
