//! Opt-in paired synthetic benchmark. The reference freezes the actual round-3 writer path:
//! unbounded Value channel, per-message to_string, two writes, one flush. Never swaps source.
use super::*;
use crate::transport::outbound_writer::run_outbound_writer;
use serde_json::json;
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

const DELTAS_PER_AGENT: usize = 256;
const BURST: usize = 32;

#[derive(Default)]
struct ReferenceStats {
    bytes: AtomicUsize,
    peak_bytes: AtomicUsize,
    frames: AtomicUsize,
    peak_frames: AtomicUsize,
    serialization_ns: Mutex<u128>,
}

struct CountWriter<W> {
    inner: W,
    writes: Arc<AtomicUsize>,
    flushes: Arc<AtomicUsize>,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for CountWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, bytes);
        if matches!(result, Poll::Ready(Ok(_))) {
            self.writes.fetch_add(1, Ordering::Relaxed);
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_flush(cx);
        if matches!(result, Poll::Ready(Ok(_))) {
            self.flushes.fetch_add(1, Ordering::Relaxed);
        }
        result
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

fn rss_high_water_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // getrusage writes exactly the platform rusage struct. RSS is process high-water, including
    // fixtures/runtime/allocator; it is not a precise attribution to this queue or sample.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    let rss = unsafe { usage.assume_init() }.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        rss
    } else {
        rss * 1024
    }
}

async fn sample(agents: usize, slow: bool, bounded: bool) -> Value {
    let mut streams = Vec::new();
    let mut expected = HashMap::new();
    let mut input_events = 0;
    let mut input_bytes = 0;
    for agent in 0..agents {
        let run = format!("run-{agent}");
        let mut text = String::new();
        let mut values = Vec::new();
        for index in 0..DELTAS_PER_AGENT {
            let fragment = format!("字😀 {index}\n`small\\delta` ");
            text.push_str(&fragment);
            values.push(super::tests::delta(&run, "stream", &fragment));
            if (index + 1) % BURST == 0 {
                values.push(json!({"jsonrpc":"2.0", "id":agent * 10000 + index,
                    "result":{"control":"ping/cancel-ack"}}));
            }
        }
        values.push(json!({"jsonrpc":"2.0","method":"agent.event","params":{
            "type":"done","runId":run,"status":"completed"}}));
        let values: Vec<_> = values
            .into_iter()
            .map(|value| {
                let size = serde_json::to_vec(&value).unwrap().len() + 1;
                input_bytes += size;
                input_events += 1;
                (value, size)
            })
            .collect();
        streams.push(values);
        expected.insert(run, text);
    }
    let rss_before = rss_high_water_bytes();
    let started = Instant::now();
    let (sink, mut source) = tokio::io::duplex(4096);
    let writes = Arc::new(AtomicUsize::new(0));
    let flushes = Arc::new(AtomicUsize::new(0));
    let writer = CountWriter {
        inner: sink,
        writes: Arc::clone(&writes),
        flushes: Arc::clone(&flushes),
    };
    let (bounded_tx, bounded_rx) = outbound_channel();
    let (reference_tx, mut reference_rx) = mpsc::unbounded_channel::<(Value, usize)>();
    let reference = Arc::new(ReferenceStats::default());
    let reference_writer = Arc::clone(&reference);
    let (_images, image_rx) = mpsc::channel(2);
    let (finish, finishing) = oneshot::channel();
    let writer_task = tokio::spawn(async move {
        if bounded {
            run_outbound_writer(writer, bounded_rx, image_rx, finishing)
                .await
                .unwrap();
        } else {
            let mut writer = writer;
            while let Some((value, size)) = reference_rx.recv().await {
                reference_writer.frames.fetch_sub(1, Ordering::Relaxed);
                reference_writer.bytes.fetch_sub(size, Ordering::Relaxed);
                let start = Instant::now();
                let encoded = value.to_string();
                *reference_writer.serialization_ns.lock().unwrap() += start.elapsed().as_nanos();
                writer.write_all(encoded.as_bytes()).await.unwrap();
                writer.write_all(b"\n").await.unwrap();
                writer.flush().await.unwrap();
            }
            writer.shutdown().await.unwrap();
        }
    });
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let enqueue_ns = Arc::new(Mutex::new(0_u128));
    let control_sends = Arc::clone(&controls);
    let reader = tokio::spawn(async move {
        let mut buffer = [0; 1024];
        let mut pending = Vec::new();
        let mut output_bytes = 0;
        let mut output_events = 0;
        let mut body = HashMap::<String, String>::new();
        let mut latencies = Vec::new();
        let mut first_character_ms = None;
        loop {
            let count = source.read(&mut buffer).await.unwrap();
            if count == 0 {
                break;
            }
            output_bytes += count;
            pending.extend_from_slice(&buffer[..count]);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let value: Value = serde_json::from_slice(&pending[..end]).unwrap();
                pending.drain(..=end);
                output_events += 1;
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    let sent: Instant = control_sends.lock().unwrap().remove(&id).unwrap();
                    latencies.push(sent.elapsed().as_secs_f64() * 1000.0);
                } else if value["params"]["type"] == "message_delta" {
                    first_character_ms.get_or_insert(started.elapsed().as_secs_f64() * 1000.0);
                    body.entry(value["params"]["runId"].as_str().unwrap().to_owned())
                        .or_default()
                        .push_str(value["params"]["delta"].as_str().unwrap());
                } else if value["params"]["type"] == "done" {
                    let run = value["params"]["runId"].as_str().unwrap();
                    assert_eq!(
                        body.get(run),
                        expected.get(run),
                        "terminal preceded tail or text changed"
                    );
                }
            }
            if slow {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
        assert!(pending.is_empty());
        assert_eq!(body, expected);
        latencies.sort_by(f64::total_cmp);
        json!({"output_events":output_events,"output_bytes":output_bytes,
            "control_p50_ms":latencies[latencies.len()/2],
            "control_p95_ms":latencies[(latencies.len()*95/100).min(latencies.len()-1)],
            "first_character_ms":first_character_ms.unwrap(),
            "total_ms":started.elapsed().as_secs_f64()*1000.0})
    });
    let mut producers = Vec::new();
    for values in streams {
        let tx = bounded_tx.clone();
        let old = reference_tx.clone();
        let controls = Arc::clone(&controls);
        let stats = Arc::clone(&reference);
        let enqueue_ns = Arc::clone(&enqueue_ns);
        producers.push(tokio::spawn(async move {
            let mut deltas = 0;
            let mut producer_ns = 0;
            for (value, size) in values {
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    controls.lock().unwrap().insert(id, Instant::now());
                }
                let before_send = Instant::now();
                if bounded {
                    tx.send(value).unwrap();
                } else {
                    let frames = stats.frames.fetch_add(1, Ordering::Relaxed) + 1;
                    let bytes = stats.bytes.fetch_add(size, Ordering::Relaxed) + size;
                    stats.peak_frames.fetch_max(frames, Ordering::Relaxed);
                    stats.peak_bytes.fetch_max(bytes, Ordering::Relaxed);
                    old.send((value, size)).unwrap();
                }
                producer_ns += before_send.elapsed().as_nanos();
                deltas += 1;
                if deltas % (BURST + 1) == 0 {
                    tokio::task::yield_now().await;
                }
            }
            *enqueue_ns.lock().unwrap() += producer_ns;
        }));
    }
    for producer in producers {
        producer.await.unwrap();
    }
    drop(reference_tx);
    if bounded {
        finish.send(()).unwrap();
    } else {
        drop(finish);
    }
    writer_task.await.unwrap();
    let mut measured = reader.await.unwrap();
    let stats = bounded_tx.stats();
    measured["mode"] = json!(if bounded {
        "bounded"
    } else {
        "frozen_unbounded"
    });
    measured["agents"] = json!(agents);
    measured["slow_consumer"] = json!(slow);
    measured["input_events"] = json!(input_events);
    measured["input_bytes"] = json!(input_bytes);
    measured["write_calls"] = json!(writes.load(Ordering::Relaxed));
    measured["flush_calls"] = json!(flushes.load(Ordering::Relaxed));
    measured["queue_peak_frames"] = json!(if bounded {
        stats.peak_frames
    } else {
        reference.peak_frames.load(Ordering::Relaxed)
    });
    measured["queue_peak_wire_bytes"] = json!(if bounded {
        stats.peak_wire_bytes
    } else {
        reference.peak_bytes.load(Ordering::Relaxed)
    });
    measured["serialization_ms"] = json!(if bounded {
        stats.serialization_ns as f64 / 1e6
    } else {
        *reference.serialization_ns.lock().unwrap() as f64 / 1e6
    });
    measured["queue_peak_buffer_bytes"] = if bounded {
        json!(stats.peak_bytes)
    } else {
        Value::Null
    };
    measured["merged_events"] = json!(if bounded { stats.merged_events } else { 0 });
    let enqueue_ms = *enqueue_ns.lock().unwrap() as f64 / 1e6;
    measured["enqueue_ms"] = json!(enqueue_ms);
    measured["enqueue_plus_serialization_ms"] = json!(
        enqueue_ms
            + if bounded {
                0.0
            } else {
                *reference.serialization_ns.lock().unwrap() as f64 / 1e6
            }
    );
    measured["rss_high_water_before_bytes"] = json!(rss_before);
    measured["rss_high_water_after_bytes"] = json!(rss_high_water_bytes());
    measured
}

#[test]
fn stream_transport_benchmark() {
    let Ok(path) = std::env::var("STREAM_TRANSPORT_BENCH_OUTPUT") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let mut rows = Vec::new();
    let selected = std::env::var("STREAM_TRANSPORT_BENCH_CASE").ok();
    runtime.block_on(async {
        for agents in [1, 4, 8] {
            for slow in [false, true] {
                for round in 0..6 {
                    // Alternate order to avoid consistently favoring the second warm runtime.
                    for bounded in if round % 2 == 0 {
                        [false, true]
                    } else {
                        [true, false]
                    } {
                        let case = format!(
                            "{agents},{},{}",
                            if slow { "slow" } else { "fast" },
                            if bounded {
                                "bounded"
                            } else {
                                "frozen_unbounded"
                            }
                        );
                        if selected.as_ref().is_some_and(|selected| selected != &case) {
                            continue;
                        }
                        let mut row = sample(agents, slow, bounded).await;
                        row["sample"] = json!(round);
                        if round > 0 {
                            rows.push(row);
                        }
                    }
                }
            }
        }
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    eprintln!(
        "stream transport paired matrix: {} measured rows saved to {path}",
        rows.len()
    );
}
