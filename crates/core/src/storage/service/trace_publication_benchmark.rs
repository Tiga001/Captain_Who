use super::*;
use crate::llm::{LlmMessage, LlmToolCall};
use crate::storage::trace_performance_metrics as metrics;
use crate::{AgentApprovalStatus, AgentToolCall, AgentToolResult};
use std::time::Instant;

fn percent95(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    values
        .get((values.len() * 95 / 100).min(values.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0)
}

#[cfg(unix)]
fn process_usage() -> (f64, u64) {
    // getrusage initializes the provided fixed-size struct and has no retained references.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
    let cpu_ms = (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.
        + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.;
    let rss = usage.ru_maxrss as u64;
    (
        cpu_ms,
        if cfg!(target_os = "macos") {
            rss
        } else {
            rss * 1024
        },
    )
}

fn publish(
    service: &StorageService,
    cursor: &mut ConversationTraceCommitCursor,
    recorder: &ConversationTraceRecorder,
    id: &str,
    mode: &str,
) {
    if mode == "incremental" {
        service
            .append_trusted_conversation_trace_publication(
                cursor,
                &recorder.publication(),
                id,
                id,
                id,
                1,
                2,
            )
            .unwrap();
        return;
    }
    let snapshot = recorder.snapshot();
    let trace = snapshot.in_progress_audit_trace(id, id, id);
    if mode == "phase_a" {
        service
            .append_conversation_trace_with_previous_projection(
                &trace,
                &snapshot.model_context_items,
                1,
                2,
            )
            .unwrap();
        return;
    }
    // Reproduce the prior Host + repository call graph on exactly the same schema and data.
    service.get_conversation_turn_trace(id).unwrap();
    service.get_conversation_model_context_log(id).unwrap();
    let started = Instant::now();
    let mut connection = service.state.connection().unwrap();
    metrics::lock_wait(started.elapsed());
    let transaction_started = Instant::now();
    let transaction = connection.transaction().unwrap();
    conversation_trace_repository::commit_trace_in_connection(&transaction, &trace, 1, 2).unwrap();
    conversation_model_context_repository::commit_items_in_connection(
        &transaction,
        id,
        id,
        &snapshot.model_context_items,
    )
    .unwrap();
    transaction.commit().unwrap();
    metrics::transaction(transaction_started.elapsed());
}

#[test]
fn trace_publication_growth_benchmark() {
    let Ok(mode) = std::env::var("MYCOPILOT_TRACE_BENCH") else {
        return;
    };
    assert!(["baseline", "phase_a", "incremental"].contains(&mode.as_str()));
    let agents = std::env::var("MYCOPILOT_TRACE_BENCH_AGENTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let bytes = std::env::var("MYCOPILOT_TRACE_BENCH_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4096);
    let sizes = std::env::var("MYCOPILOT_TRACE_BENCH_STEPS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(|part| part.parse::<usize>().unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![10, 100, 500]);
    for steps in sizes {
        let (_directory, service, _, _) = fixture();
        for agent in 0..agents {
            let id = format!("bench-{agent}");
            service.state.connection().unwrap().execute("INSERT INTO conversations(id,title,created_at,updated_at) VALUES (?1,'Bench',1,1)", [&id]).unwrap();
            service.state.connection().unwrap().execute("INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES (?1,?1,'assistant','','pending',1,0)", [&id]).unwrap();
        }
        let service = Arc::new(service);
        let barrier = Arc::new(std::sync::Barrier::new(agents));
        let started = Instant::now();
        let initial_cpu = process_usage().0;
        let results = std::thread::scope(|scope| {
            (0..agents)
                .map(|agent| {
                    let service = service.clone();
                    let barrier = barrier.clone();
                    let mode = mode.clone();
                    scope.spawn(move || {
                        let id = format!("bench-{agent}");
                        let mut recorder = ConversationTraceRecorder::default();
                        let mut cursor = ConversationTraceCommitCursor::default();
                        let mut durations = Vec::new();
                        let output = "0123456789abcdef".repeat(bytes.div_ceil(16));
                        metrics::start();
                        barrier.wait();
                        for step in 0..steps {
                            let call = AgentToolCall {
                                id: format!("call-{step}"),
                                tool: "read_file".into(),
                                args: serde_json::json!({"path": "fixture.txt"}),
                                approval_status: AgentApprovalStatus::NotRequired,
                                reason: None,
                            };
                            if mode == "baseline" {
                                recorder = recorder.clone();
                            }
                            let sequence = recorder.record_tool_call(&call).unwrap();
                            recorder
                                .record_model_message(
                                    sequence,
                                    0,
                                    &LlmMessage::assistant(
                                        "",
                                        vec![LlmToolCall {
                                            id: call.id.clone(),
                                            name: call.tool.clone(),
                                            args: call.args.clone(),
                                        }],
                                    ),
                                )
                                .unwrap();
                            let instant = Instant::now();
                            publish(&service, &mut cursor, &recorder, &id, &mode);
                            durations.push(instant.elapsed().as_nanos() as u64);
                            if mode == "baseline" {
                                recorder = recorder.clone();
                            }
                            let result = AgentToolResult {
                                call_id: call.id.clone(),
                                tool: call.tool.clone(),
                                ok: true,
                                result: Some(serde_json::json!({ "content": output })),
                                error: None,
                                exact_archive_file: None,
                            };
                            let sequence = recorder.record_tool_result(&call, &result).unwrap();
                            recorder
                                .record_model_message(
                                    sequence,
                                    0,
                                    &LlmMessage::tool_result(call.id, output.clone(), false),
                                )
                                .unwrap();
                            let instant = Instant::now();
                            publish(&service, &mut cursor, &recorder, &id, &mode);
                            durations.push(instant.elapsed().as_nanos() as u64);
                        }
                        (metrics::finish(), durations)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let wall_ms = started.elapsed().as_secs_f64() * 1000.;
        let (cpu_ms, peak_rss_bytes) = process_usage();
        let mut total = metrics::TraceReadMetrics::default();
        let mut durations = Vec::new();
        for (value, samples) in results {
            total.trace_loads += value.trace_loads;
            total.trace_bytes += value.trace_bytes;
            total.context_loads += value.context_loads;
            total.context_bytes += value.context_bytes;
            total.decompressions += value.decompressions;
            total.lock_wait_ns.extend(value.lock_wait_ns);
            total.transaction_ns.extend(value.transaction_ns);
            durations.extend(samples);
        }
        println!("TRACE_BENCH mode={mode} agents={agents} steps={steps} output_bytes={bytes} wall_ms={wall_ms:.3} cpu_ms={:.3} peak_rss_bytes={peak_rss_bytes} commit_p95_us={:.3} lock_p95_us={:.3} transaction_p95_us={:.3} trace_loads={} context_loads={} decompress={} history_bytes={}", cpu_ms-initial_cpu, percent95(&mut durations) as f64 / 1000., percent95(&mut total.lock_wait_ns) as f64 / 1000., percent95(&mut total.transaction_ns) as f64 / 1000., total.trace_loads, total.context_loads, total.decompressions, total.trace_bytes + total.context_bytes);
    }
}
