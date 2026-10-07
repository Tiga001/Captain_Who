//! Reproducible mixed transport load, independent of providers and the user's database.
//! These measurements attribute queue pressure, not application CPU or end-to-end UI latency.
use super::*;
use crate::transport::outbound_writer::run_outbound_writer;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};

fn child_delta(agent: usize, sequence: usize) -> Value {
    json!({"jsonrpc":"2.0","method":"agent.collaboration.childEvent","params":{
        "schemaVersion":1,"rootAgentId":"root-agent","rootConversationId":"root-conversation",
        "agentId":format!("child-agent-{agent:04}"),"conversationId":format!("child-conversation-{agent:04}"),
        "runId":format!("child-run-{agent:04}"),"assistantMessageId":format!("child-assistant-{agent:04}"),
        "streamCursor":{"generation":format!("generation-{agent:04}"),"sequence":sequence},
        "event":{"type":"message_delta","runId":format!("child-run-{agent:04}"),"streamId":"stream",
            "delta":"测试碎片 😀 \"quoted\"\n"}}})
}

fn runtime_snapshot(mail: usize, sequence: usize) -> Value {
    let inputs: Vec<_> = (0..mail).map(|index| json!({
        "id":format!("input-{index}"),"instanceId":"organization","nodeId":format!("node-{}",index%16),
        "conversationId":format!("conversation-{}",index%16),"executionVersion":"version",
        "content":"","messages":[{"id":format!("mail-{index}"),"instanceId":"organization",
            "workflowName":"Organization","sourceNodeId":"sender","sourceNodeName":"Sender",
            "sourceConversationId":"source-conversation","sourceConversationTitle":"Source",
            "targetNodeId":format!("node-{}",index%16),"targetNodeName":"Recipient",
            "targetConversationId":format!("conversation-{}",index%16),"targetConversationTitle":"Recipient conversation",
            "replyToMessageId":null,"content":"","createdAt":1}],
        "mailStatus":"pending","status":"pending","runId":null,"deliveryId":null,"createdAt":1,"error":null
    })).collect();
    json!({"jsonrpc":"2.0","method":"agent.workflows.runtime.changed","params":{
        "instanceId":"organization","sequence":sequence,"inputs":inputs,"events":[],
        "pausedConversationIds":[],"inputRuns":[]}})
}

fn runtime_summary(mail: usize, sequence: usize) -> Value {
    // Same pending envelopes as runtime_snapshot. This workload has no accepted deliveries or
    // management events, so only the per-node counts belong in its compact wire projection.
    let pending = (0..16)
        .filter_map(|node| {
            let count = (node..mail).step_by(16).count();
            (count > 0).then(|| json!({"nodeId":format!("node-{node}"),"count":count}))
        })
        .collect::<Vec<_>>();
    json!({"jsonrpc":"2.0","method":"agent.workflows.runtime.changed","params":{
        "instanceId":"organization","sequence":sequence,"inputs":[],"events":[],
        "pausedConversationIds":[],"inputRuns":[],"summary":{
            "pendingByNode":pending,"conversationChanges":[],"structureRevision":0}}})
}

#[test]
fn mixed_workload_fixtures_are_valid_and_attribute_byte_pressure() {
    let snapshot = runtime_snapshot(1024, 1);
    let parsed: mycopilot_core::workflow_execution::RuntimeSnapshot =
        serde_json::from_value(snapshot["params"].clone()).unwrap();
    assert_eq!(parsed.inputs.len(), 1024);
    let (sender, _receiver) = outbound_channel();
    let mut accepted = 0;
    while sender.send(snapshot.clone()).is_ok() {
        accepted += 1;
        assert!(accepted < 100);
    }
    let failure = sender
        .shared
        .state
        .lock()
        .unwrap()
        .failure_snapshot
        .clone()
        .unwrap();
    assert!(failure.byte_limit);
    assert!(!failure.frame_limit);
    assert_eq!(failure.category, "workflow.runtime");
    assert!(sender.stats().peak_bytes <= DATA_BYTES + CONTROL_RESERVE_BYTES);
}

#[test]
fn summary_workload_preserves_counts_without_repeating_envelopes() {
    for mail in [128, 512, 1024] {
        let full = runtime_snapshot(mail, 128);
        let compact = runtime_summary(mail, 128);
        let snapshot: mycopilot_core::workflow_execution::RuntimeSnapshot =
            serde_json::from_value(compact["params"].clone()).unwrap();
        assert_eq!(snapshot.sequence, 128);
        assert!(snapshot.inputs.is_empty());
        let summary = snapshot.summary.unwrap();
        assert_eq!(
            summary
                .pending_by_node
                .iter()
                .map(|row| row.count)
                .sum::<u64>(),
            mail as u64
        );
        assert!(summary.conversation_changes.is_empty());
        assert!(
            serde_json::to_vec(&compact).unwrap().len() * 20
                < serde_json::to_vec(&full).unwrap().len()
        );
    }
}

async fn sample(children: usize, mail: usize, slow: bool, summary: bool) -> Value {
    let (sender, receiver) = outbound_channel();
    let (sink, source) = tokio::io::duplex(16 * 1024);
    let (images, image_receiver) = mpsc::channel(1);
    let (finish, finishing) = oneshot::channel();
    let writer = tokio::spawn(run_outbound_writer(
        sink,
        receiver,
        image_receiver,
        finishing,
    ));
    let started = Instant::now();
    let sent_controls = Arc::new(Mutex::new(BTreeMap::<usize, Instant>::new()));
    let controls = Arc::clone(&sent_controls);
    let reader = tokio::spawn(async move {
        let mut reader = BufReader::new(source);
        let mut line = Vec::new();
        let mut bytes = 0;
        let mut frames = 0;
        let mut truncated_tail_bytes = 0;
        let mut latencies = Vec::new();
        let mut positions = BTreeMap::new();
        let mut overloaded = false;
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line).await.unwrap();
            if read == 0 {
                break;
            }
            bytes += read;
            // A failed connection can reach its drain deadline mid-frame. Record that tail;
            // the caller permits it only after overload, never for a healthy/summary workload.
            if line.last() != Some(&b'\n') {
                truncated_tail_bytes = read;
                break;
            }
            frames += 1;
            let value: Value = serde_json::from_slice(&line).unwrap();
            if let Some(id) = value["id"].as_u64() {
                if let Some(sent) = controls.lock().unwrap().remove(&(id as usize)) {
                    latencies.push(sent.elapsed().as_secs_f64() * 1000.0);
                }
            } else if value["method"] == "agent.collaboration.childEvent" {
                let params = &value["params"];
                let position = positions
                    .entry(params["agentId"].as_str().unwrap().to_owned())
                    .or_insert(0);
                assert_eq!(params["streamCursor"]["sequence"], *position + 1);
                *position += 1;
            } else if value["error"]["data"]["code"] == "outbound_overloaded" {
                overloaded = true;
            }
            if slow {
                tokio::time::sleep(Duration::from_micros(100)).await;
            }
        }
        latencies.sort_by(f64::total_cmp);
        json!({"receivedBytes":bytes,"receivedFrames":frames,"receivedOverload":overloaded,
            "truncatedTailBytes":truncated_tail_bytes,
            "controlP95Ms":latencies.get(latencies.len()*95/100),"totalMs":started.elapsed().as_secs_f64()*1000.0})
    });
    let mut attempted_bytes = BTreeMap::<&str, usize>::new();
    let mut accepted = 0;
    'produce: for round in 1..=128 {
        let mut values = (0..children)
            .map(|agent| child_delta(agent, round))
            .collect::<Vec<_>>();
        if round % 8 == 0 {
            values.push(if summary {
                runtime_summary(mail, round)
            } else {
                runtime_snapshot(mail, round)
            });
        }
        if round % 32 == 0 {
            values.push(
                json!({"jsonrpc":"2.0","id":round+10000,"result":{"history":"x".repeat(256*1024)}}),
            );
        }
        sent_controls.lock().unwrap().insert(round, Instant::now());
        values.push(json!({"jsonrpc":"2.0","id":round,"result":"pong"}));
        for value in values {
            *attempted_bytes
                .entry(diagnostic_category(&value, is_body_delta(&value)))
                .or_default() += serde_json::to_vec(&value).unwrap().len() + 1;
            if sender.send(value).is_err() {
                break 'produce;
            }
            accepted += 1;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    drop(images);
    // Finishing production is not application shutdown. Let a healthy slow consumer catch up
    // before starting the writer's separate five-second shutdown deadline.
    if !sender.is_closed() {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if sent_controls.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("healthy mixed workload did not drain");
    }
    let _ = finish.send(());
    let writer_result = writer.await.unwrap();
    let mut report = reader.await.unwrap();
    let state = sender.shared.state.lock().unwrap();
    report["children"] = json!(children);
    report["mail"] = json!(mail);
    report["slow"] = json!(slow);
    report["projection"] = json!(if summary { "summary" } else { "full" });
    report["acceptedEvents"] = json!(accepted);
    report["attemptedBytesByCategory"] = json!(attempted_bytes);
    report["queuePeakBytes"] = json!(state.stats.peak_bytes);
    report["queuePeakFrames"] = json!(state.stats.peak_frames);
    report["limits"] = json!({
        "dataBytes":sender.shared.limits.data_bytes,
        "controlBytes":sender.shared.limits.control_bytes,
        "frames":sender.shared.limits.frames,
        "controlFrames":sender.shared.limits.control_frames
    });
    report["serializationMs"] = json!(state.stats.serialization_ns as f64 / 1e6);
    report["writerCompleted"] = json!(writer_result.is_ok());
    let writer_timed_out = writer_result
        .as_ref()
        .err()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::TimedOut);
    report["writerTimedOut"] = json!(writer_timed_out);
    report["overloaded"] = json!(state.overloaded);
    assert!(state.stats.peak_bytes <= DATA_BYTES + CONTROL_RESERVE_BYTES);
    assert!(state.stats.peak_frames <= MAX_FRAMES);
    assert!(
        report["truncatedTailBytes"] == 0 || (state.overloaded && writer_timed_out),
        "healthy mixed workload ended with a truncated frame: {report}"
    );
    if summary {
        assert!(
            !state.overloaded,
            "compact mixed workload exhausted configured queue budgets: {report}"
        );
        assert!(
            writer_result.is_ok(),
            "compact mixed workload failed to drain: {report}"
        );
        assert_eq!(report["receivedFrames"], accepted);
    }
    report
}

#[test]
fn organization_child_mixed_transport_benchmark() {
    let Ok(path) = std::env::var("CORE_MIXED_BENCH_OUTPUT") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let rows = runtime.block_on(async {
        let mut rows = Vec::new();
        for (children, mail) in [(1, 128), (16, 512), (50, 1024)] {
            for slow in [false, true] {
                for summary in [false, true] {
                    rows.push(sample(children, mail, slow, summary).await);
                }
            }
        }
        rows
    });
    std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
}
