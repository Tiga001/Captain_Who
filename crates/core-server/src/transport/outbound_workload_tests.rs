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

async fn sample(children: usize, mail: usize, slow: bool) -> Value {
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
        let mut lines = BufReader::new(source).lines();
        let mut bytes = 0;
        let mut frames = 0;
        let mut latencies = Vec::new();
        let mut positions = BTreeMap::new();
        let mut overloaded = false;
        while let Some(line) = lines.next_line().await.unwrap() {
            bytes += line.len() + 1;
            frames += 1;
            let value: Value = serde_json::from_str(&line).unwrap();
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
            "controlP95Ms":latencies.get(latencies.len()*95/100),"totalMs":started.elapsed().as_secs_f64()*1000.0})
    });
    let mut attempted_bytes = BTreeMap::<&str, usize>::new();
    let mut accepted = 0;
    'produce: for round in 1..=128 {
        let mut values = (0..children)
            .map(|agent| child_delta(agent, round))
            .collect::<Vec<_>>();
        if round % 8 == 0 {
            values.push(runtime_snapshot(mail, round));
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
    report["acceptedEvents"] = json!(accepted);
    report["attemptedBytesByCategory"] = json!(attempted_bytes);
    report["queuePeakBytes"] = json!(state.stats.peak_bytes);
    report["queuePeakFrames"] = json!(state.stats.peak_frames);
    report["serializationMs"] = json!(state.stats.serialization_ns as f64 / 1e6);
    report["writerCompleted"] = json!(writer_result.is_ok());
    report["overloaded"] = json!(state.overloaded);
    assert!(state.stats.peak_bytes <= DATA_BYTES + CONTROL_RESERVE_BYTES);
    assert!(state.stats.peak_frames <= MAX_FRAMES);
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
                rows.push(sample(children, mail, slow).await);
            }
        }
        rows
    });
    std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
}
