//! Frozen pre-change reference: workflow/human-interaction handler runs inline before the reader
//! can dispatch the next control request. Never swap source trees or touch the application's DB.
use super::*;
use std::time::Instant;

#[derive(Debug, Clone, Copy, Serialize)]
struct Sample {
    queue_ms: f64,
    handler_ms: f64,
    db_lock_ms: f64,
    total_ms: f64,
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn control_sample(arrived: Instant, database: Option<&Mutex<rusqlite::Connection>>) -> Sample {
    let started = Instant::now();
    let mut db_lock = Duration::ZERO;
    if let Some(database) = database {
        let lock_start = Instant::now();
        let connection = database.lock().unwrap();
        db_lock = lock_start.elapsed();
        let value: i64 = connection
            .query_row("SELECT 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 1);
    }
    let completed = Instant::now();
    Sample {
        queue_ms: milliseconds(started.duration_since(arrived)),
        handler_ms: milliseconds(completed.duration_since(started)),
        db_lock_ms: milliseconds(db_lock),
        total_ms: milliseconds(completed.duration_since(arrived)),
    }
}

async fn frozen_inline_reference(shared_db: bool) -> Sample {
    tokio::task::spawn_blocking(move || {
        let database = Mutex::new(rusqlite::Connection::open_in_memory().unwrap());
        let arrived = Instant::now();
        // Same 20 ms slow read as the dispatcher case. The original reader cannot reach cancel
        // until this handler returns; consequently DB wait is charged to intake queue delay.
        {
            let _guard = shared_db.then(|| database.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
        control_sample(arrived, shared_db.then_some(&database))
    })
    .await
    .unwrap()
}

async fn bounded_dispatch_sample(shared_db: bool) -> Sample {
    let (outbound, mut responses) = crate::transport::outbound_channel();
    let rpc = RpcRequestDispatcher::new(outbound);
    let database = Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
    let reader_database = Arc::clone(&database);
    let (started, ready) = oneshot::channel();
    rpc.try_submit(
        RpcDispatchClass::Read,
        JsonRpcId::Number(1),
        None,
        move || {
            let _guard = shared_db.then(|| reader_database.lock().unwrap());
            started.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            response_success(JsonRpcId::Number(1), true)
        },
    )
    .unwrap();
    ready.await.unwrap();
    let arrived = Instant::now();
    let (sample_tx, sample_rx) = oneshot::channel();
    rpc.try_submit(
        RpcDispatchClass::Control,
        JsonRpcId::Number(2),
        Some("run:synthetic".into()),
        move || {
            let sample = control_sample(arrived, shared_db.then_some(database.as_ref()));
            sample_tx.send(sample).unwrap();
            response_success(JsonRpcId::Number(2), true)
        },
    )
    .unwrap();
    let sample = sample_rx.await.unwrap();
    rpc.shutdown().await.unwrap();
    let mut ids = Vec::new();
    while let Some(response) = responses.recv().await {
        ids.push(response["id"].as_i64().unwrap());
    }
    ids.sort();
    assert_eq!(ids, [1, 2]);
    sample
}

fn percentile(mut values: Vec<f64>, p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((values.len() as f64 * p).ceil() as usize).saturating_sub(1)]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_dispatch_latency_benchmark() {
    if std::env::var_os("RPC_BENCH_OUTPUT").is_none() {
        return;
    }
    let mut csv =
        String::from("scenario,implementation,sample,queue_ms,handler_ms,db_lock_ms,total_ms\n");
    for shared_db in [false, true] {
        let scenario = if shared_db {
            "shared_db_lock"
        } else {
            "no_shared_lock"
        };
        for improved in [false, true] {
            let implementation = if improved { "bounded" } else { "frozen_inline" };
            let mut samples = Vec::new();
            for number in 0..35 {
                let sample = if improved {
                    bounded_dispatch_sample(shared_db).await
                } else {
                    frozen_inline_reference(shared_db).await
                };
                if number < 5 {
                    continue;
                }
                csv.push_str(&format!(
                    "{scenario},{implementation},{},{:.6},{:.6},{:.6},{:.6}\n",
                    number - 5,
                    sample.queue_ms,
                    sample.handler_ms,
                    sample.db_lock_ms,
                    sample.total_ms
                ));
                samples.push(sample);
            }
            let metrics = [
                (
                    "queue_ms",
                    samples.iter().map(|sample| sample.queue_ms).collect(),
                ),
                (
                    "handler_ms",
                    samples.iter().map(|sample| sample.handler_ms).collect(),
                ),
                (
                    "db_lock_ms",
                    samples.iter().map(|sample| sample.db_lock_ms).collect(),
                ),
                (
                    "total_ms",
                    samples.iter().map(|sample| sample.total_ms).collect(),
                ),
            ]
            .into_iter()
            .map(|(name, values): (&str, Vec<f64>)| {
                (
                    name.to_string(),
                    json!({"p50":percentile(values.clone(), 0.5), "p95":percentile(values, 0.95)}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
            println!(
                "{}",
                json!({"scenario":scenario,"implementation":implementation,"samples":samples.len(),"metrics":metrics})
            );
        }
    }
    if let Ok(path) = std::env::var("RPC_BENCH_OUTPUT") {
        std::fs::write(path, csv).unwrap();
    }
}

// This benchmark isolates admission/dispatch contention, not the production approval handler
// or Renderer. All state is synthetic and the SQLite connection exists only for this sample.
async fn approval_backlog_sample(background_agents: usize, improved: bool) -> Sample {
    let (outbound, mut responses) = crate::transport::outbound_channel();
    let rpc = RpcRequestDispatcher::new(outbound);
    let database = Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
    database
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TABLE decisions (id INTEGER PRIMARY KEY, status TEXT NOT NULL);
         INSERT INTO decisions VALUES (1, 'pending');",
        )
        .unwrap();
    let (started, ready) = oneshot::channel();
    rpc.try_submit(
        RpcDispatchClass::Write,
        JsonRpcId::Number(1),
        Some("conversation:background-0".into()),
        move || {
            started.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(10));
            response_success(JsonRpcId::Number(1), true)
        },
    )
    .unwrap();
    ready.await.unwrap();
    for agent in 1..background_agents {
        let id = (agent + 1) as i64;
        rpc.try_submit(
            RpcDispatchClass::Write,
            JsonRpcId::Number(id),
            Some(format!("conversation:background-{agent}")),
            move || {
                std::thread::sleep(Duration::from_millis(2));
                response_success(JsonRpcId::Number(id), true)
            },
        )
        .unwrap();
    }
    let request: JsonRpcRequest = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 1000,
        "method": AGENT_APPROVE_ACTION_METHOD,
        "params": {"runId": "synthetic-approval", "actionId": "synthetic-action"}
    }))
    .unwrap();
    let class = if improved {
        rpc_dispatch_class(&request)
    } else {
        RpcDispatchClass::Write
    };
    let (sample_tx, sample_rx) = oneshot::channel();
    let arrived = Instant::now();
    let decision_database = Arc::clone(&database);
    rpc.try_submit(
        class,
        request.id.clone(),
        rpc_ordering_key(&request),
        move || {
            let started = Instant::now();
            let connection = decision_database.lock().unwrap();
            let acquired = Instant::now();
            assert_eq!(connection.execute(
            "UPDATE decisions SET status = 'approved' WHERE id = 1 AND status = 'pending'", [],
        ).unwrap(), 1);
            let completed = Instant::now();
            sample_tx
                .send(Sample {
                    queue_ms: milliseconds(started.duration_since(arrived)),
                    handler_ms: milliseconds(completed.duration_since(started)),
                    db_lock_ms: milliseconds(acquired.duration_since(started)),
                    total_ms: milliseconds(completed.duration_since(arrived)),
                })
                .unwrap();
            response_success(JsonRpcId::Number(1000), true)
        },
    )
    .unwrap();
    let sample = sample_rx.await.unwrap();
    rpc.shutdown().await.unwrap();
    let mut ids = Vec::new();
    while let Some(response) = responses.recv().await {
        assert!(response.get("error").is_none());
        ids.push(response["id"].as_i64().unwrap());
    }
    ids.sort();
    let expected = (1..=background_agents as i64)
        .chain([1000])
        .collect::<Vec<_>>();
    assert_eq!(ids, expected);
    assert_eq!(
        database
            .lock()
            .unwrap()
            .query_row("SELECT status FROM decisions WHERE id = 1", [], |row| {
                row.get::<_, String>(0)
            },)
            .unwrap(),
        "approved"
    );
    sample
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_dispatch_latency_benchmark() {
    let Ok(path) = std::env::var("RPC_APPROVAL_BENCH_OUTPUT") else {
        return;
    };
    let mut csv = String::from(
        "background_agents,implementation,sample,queue_ms,handler_ms,db_lock_ms,total_ms\n",
    );
    for agents in [8, 16, 32] {
        for improved in [false, true] {
            let implementation = if improved {
                "priority"
            } else {
                "serialized_write_reference"
            };
            let mut samples = Vec::new();
            for number in 0..23 {
                let sample = approval_backlog_sample(agents, improved).await;
                if number < 3 {
                    continue;
                }
                csv.push_str(&format!(
                    "{agents},{implementation},{},{:.6},{:.6},{:.6},{:.6}\n",
                    number - 3,
                    sample.queue_ms,
                    sample.handler_ms,
                    sample.db_lock_ms,
                    sample.total_ms
                ));
                samples.push(sample.total_ms);
            }
            println!(
                "{}",
                json!({
                    "scenario": "approval_unrelated_write_backlog", "background_agents": agents,
                    "implementation": implementation, "samples": samples.len(),
                    "total_ms": {"p50": percentile(samples.clone(), 0.5), "p95": percentile(samples, 0.95)}
                })
            );
        }
    }
    std::fs::write(path, csv).unwrap();
}
