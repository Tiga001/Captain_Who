use super::*;
use crate::transport::outbound_writer::run_outbound_writer_with_timeout;
use serde_json::json;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

pub(super) fn delta(run: &str, stream: &str, text: &str) -> Value {
    json!({"jsonrpc":"2.0", "method":"agent.event", "params":{
        "type":"message_delta", "runId":run, "streamId":stream, "delta":text }})
}

fn event(kind: &str) -> Value {
    json!({"jsonrpc":"2.0", "method":"agent.event", "params":{"type":kind,"runId":"run"}})
}

#[tokio::test]
async fn adjacent_delta_merge_preserves_unicode_escaping_empty_parts_and_all_boundaries() {
    let (sender, mut receiver) = outbound_channel();
    let fragments = ["", "第一行\n", "```rs\n", "let x = \"😀\\\";\n", "```", ""];
    for fragment in fragments {
        sender.send(delta("run", "a", fragment)).unwrap();
    }
    for boundary in [
        "tool_call",
        "tool_result",
        "message_stream_reset",
        "message_stream_committed",
        "done",
        "error",
        "cancelled",
    ] {
        sender.send(event(boundary)).unwrap();
        sender.send(delta("run", "a", boundary)).unwrap();
    }
    sender.send(delta("different-run", "a", "other")).unwrap();
    sender
        .send(delta("run", "different-stream", "other"))
        .unwrap();
    drop(sender);
    assert_eq!(
        receiver.recv().await.unwrap(),
        delta("run", "a", &fragments.concat())
    );
    for boundary in [
        "tool_call",
        "tool_result",
        "message_stream_reset",
        "message_stream_committed",
        "done",
        "error",
        "cancelled",
    ] {
        assert_eq!(receiver.recv().await.unwrap(), event(boundary));
        assert_eq!(receiver.recv().await.unwrap(), delta("run", "a", boundary));
    }
    assert_eq!(
        receiver.recv().await.unwrap(),
        delta("different-run", "a", "other")
    );
    assert_eq!(
        receiver.recv().await.unwrap(),
        delta("run", "different-stream", "other")
    );
    assert!(receiver.recv().await.is_none());
}

#[tokio::test]
async fn observer_cursor_and_unknown_stage_fields_are_never_merged() {
    let (sender, mut receiver) = outbound_channel();
    let mut messages = Vec::new();
    for cursor in 1..=3 {
        let observer = json!({"jsonrpc":"2.0","method":"agent.collaboration.observerEvent","params":{
            "agentId":"child", "rootAgentId":"root", "conversationId":"child-conversation",
            "streamCursor":cursor, "event":delta("run","a","x")["params"]}});
        sender.send(observer.clone()).unwrap();
        messages.push(observer);
    }
    for _ in 0..2 {
        let mut unknown_stage = delta("run", "a", "x");
        unknown_stage["params"]["stage"] = json!("future-stage");
        sender.send(unknown_stage.clone()).unwrap();
        messages.push(unknown_stage);
    }
    for metadata in ["first", "second"] {
        let mut unknown_envelope = delta("run", "a", "same-body");
        unknown_envelope["extra"] = json!({"delta": metadata});
        sender.send(unknown_envelope.clone()).unwrap();
        messages.push(unknown_envelope);
    }
    drop(sender);
    for expected in messages {
        assert_eq!(receiver.recv().await.unwrap(), expected);
    }
    assert!(receiver.recv().await.is_none());
}

#[tokio::test]
async fn byte_and_count_budgets_reserve_control_space_and_fail_the_entire_connection() {
    let body = delta("run", "a", &"x".repeat(80));
    let body_size = serde_json::to_vec(&body).unwrap().len() + 1;
    let (sender, mut receiver) = outbound_channel_with_limits(OutboundLimits {
        data_bytes: body_size * 2,
        control_bytes: 256,
        frames: 3,
        control_frames: 1,
        merge_bytes: 0,
    });
    sender.send(body.clone()).unwrap();
    sender.send(body.clone()).unwrap();
    let control = json!({"jsonrpc":"2.0","id":1,"result":"pong"});
    sender.send(control.clone()).unwrap();
    assert!(sender.send(body.clone()).is_err());
    tokio::time::timeout(Duration::from_millis(50), sender.failed())
        .await
        .unwrap();
    assert!(
        sender.send(control.clone()).is_err(),
        "no healthy-looking stream after a missing delta"
    );
    assert_eq!(receiver.recv().await.unwrap(), body);
    assert_eq!(receiver.recv().await.unwrap(), body);
    assert_eq!(receiver.recv().await.unwrap(), control);
    assert!(receiver.recv().await.is_none());
    assert!(sender.stats().peak_bytes <= body_size * 2 + 256);
    assert_eq!(sender.stats().peak_frames, 3);
    assert!(overload_frame().len() < 512);
}

#[tokio::test]
async fn command_output_and_observer_progress_use_data_budget_without_merging() {
    for (method, kind) in [
        ("agent.event", "command_output"),
        ("agent.collaboration.observerEvent", "command_output"),
        ("agent.collaboration.observerEvent", "tool_input_progress"),
    ] {
        let event = json!({"type":kind,"runId":"run","delta":"stdout\n"});
        let params = if method == "agent.event" {
            event
        } else {
            json!({"streamCursor":1,"event":event})
        };
        let body = json!({"jsonrpc":"2.0","method":method,"params":params});
        let bytes = serde_json::to_vec(&body).unwrap().len() + 1;
        let (sender, mut receiver) = outbound_channel_with_limits(OutboundLimits {
            data_bytes: bytes * 2,
            control_bytes: 256,
            frames: 3,
            control_frames: 1,
            ..OutboundLimits::default()
        });
        sender.send(body.clone()).unwrap();
        sender.send(body.clone()).unwrap();
        sender
            .send(json!({"jsonrpc":"2.0","id":1,"result":"stopped"}))
            .unwrap();
        assert!(sender.send(body.clone()).is_err());
        assert_eq!(receiver.recv().await.unwrap(), body);
        assert_eq!(receiver.recv().await.unwrap(), body);
        assert_eq!(receiver.recv().await.unwrap()["id"], 1);
        assert_eq!(sender.stats().merged_events, 0);
    }
}

#[tokio::test]
async fn one_oversized_response_is_leased_until_its_owned_frame_is_released() {
    let (sender, mut receiver) = outbound_channel_with_limits(OutboundLimits {
        data_bytes: 128,
        control_bytes: 128,
        ..OutboundLimits::default()
    });
    let response = json!({"jsonrpc":"2.0","id":1,"result":"x".repeat(4096)});
    sender.send(response.clone()).unwrap();
    let frame = receiver.recv_frame().await.unwrap();
    assert!(frame.wire.len() > 4096);
    assert_eq!(
        sender.stats().peak_bytes,
        0,
        "oversized lease is accounted separately"
    );
    drop(frame);
    sender.send(response.clone()).unwrap();
    assert!(
        sender.send(response).is_err(),
        "a second queued/in-flight oversized response overloads"
    );
}

#[tokio::test]
async fn large_terminal_keeps_its_existing_payload_without_a_new_size_cap_or_fifo_reordering() {
    let (sender, mut receiver) = outbound_channel_with_limits(OutboundLimits {
        data_bytes: 512,
        control_bytes: 128,
        ..OutboundLimits::default()
    });
    let tail = delta("run", "s", "尾部😀");
    let mut terminal = event("done");
    terminal["params"]["content"] = json!("完整终态\n".repeat(4096));
    sender.send(tail.clone()).unwrap();
    sender.send(terminal.clone()).unwrap();
    sender
        .send(json!({"jsonrpc":"2.0","id":9,"result":"shutdown"}))
        .unwrap();
    assert_eq!(receiver.recv().await.unwrap(), tail);
    assert_eq!(receiver.recv().await.unwrap(), terminal);
    assert_eq!(receiver.recv().await.unwrap()["id"], 9);
}

#[derive(Clone, Default)]
struct RecordingWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
    writes: Arc<AtomicUsize>,
    flushes: Arc<AtomicUsize>,
}

impl AsyncWrite for RecordingWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        self.writes.fetch_add(1, Ordering::Relaxed);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.flushes.fetch_add(1, Ordering::Relaxed);
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn writer_batches_ready_frames_and_flushes_tail_before_terminal_and_shutdown() {
    let (sender, receiver) = outbound_channel();
    let mut expected = Vec::new();
    for i in 0..60 {
        let value = delta(&format!("run-{i}"), "stream", "最后一段\n");
        sender.send(value.clone()).unwrap();
        expected.push(value);
    }
    sender.send(event("done")).unwrap();
    expected.push(event("done"));
    let (images, image_rx) = mpsc::channel(2);
    let (finish, finishing) = oneshot::channel();
    let writer = RecordingWriter::default();
    let recorded = writer.clone();
    finish.send(()).unwrap();
    run_outbound_writer_with_timeout(
        writer,
        receiver,
        image_rx,
        finishing,
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(recorded.writes.load(Ordering::Relaxed), 1);
    assert_eq!(recorded.flushes.load(Ordering::Relaxed), 1);
    let bytes = recorded.bytes.lock().unwrap();
    let actual: Vec<Value> = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(actual, expected);
    assert!(sender.is_closed());
    drop(images);
}

#[tokio::test]
async fn slow_stream_first_character_has_no_coalescing_timer() {
    let (sender, receiver) = outbound_channel();
    let (writer, mut reader) = tokio::io::duplex(4096);
    let (images, image_rx) = mpsc::channel(2);
    let (finish, finishing) = oneshot::channel();
    let task = tokio::spawn(run_outbound_writer_with_timeout(
        writer,
        receiver,
        image_rx,
        finishing,
        Duration::from_secs(1),
    ));
    sender.send(delta("run", "stream", "首")).unwrap();
    let mut bytes = vec![0; 512];
    let count = tokio::time::timeout(Duration::from_millis(50), reader.read(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes[..count]).unwrap();
    assert_eq!(value["params"]["delta"], "首");
    finish.send(()).unwrap();
    task.await.unwrap().unwrap();
    drop(images);
}

#[tokio::test]
async fn overload_drains_accepted_fifo_then_emits_independent_failure_frame() {
    let (sender, receiver) = outbound_channel_with_limits(OutboundLimits {
        data_bytes: 200,
        control_bytes: 128,
        frames: 4,
        control_frames: 1,
        merge_bytes: 0,
    });
    let accepted = delta("run", "s", "x");
    sender.send(accepted.clone()).unwrap();
    assert!(sender.send(delta("run", "s", &"x".repeat(1000))).is_err());
    let writer = RecordingWriter::default();
    let recorded = writer.clone();
    let (_images, image_rx) = mpsc::channel(2);
    let (_finish, finishing) = oneshot::channel();
    let result = run_outbound_writer_with_timeout(
        writer,
        receiver,
        image_rx,
        finishing,
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::OutOfMemory);
    let bytes = recorded.bytes.lock().unwrap();
    let lines: Vec<Value> = bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], accepted);
    assert_eq!(lines[1]["id"], Value::Null);
    assert_eq!(lines[1]["error"]["code"], -32002);
    assert_eq!(lines[1]["error"]["data"]["code"], "outbound_overloaded");
}

#[tokio::test]
async fn disconnect_wakes_admission_even_when_producers_ignore_send_errors() {
    let (sender, receiver) = outbound_channel();
    let (writer, reader) = tokio::io::duplex(64);
    drop(reader);
    let (_images, image_rx) = mpsc::channel(2);
    let (_finish, finishing) = oneshot::channel();
    let task = tokio::spawn(run_outbound_writer_with_timeout(
        writer,
        receiver,
        image_rx,
        finishing,
        Duration::from_secs(1),
    ));
    sender.send(delta("run", "s", "x")).unwrap();
    tokio::time::timeout(Duration::from_secs(1), sender.failed())
        .await
        .unwrap();
    assert!(task.await.unwrap().is_err());
    assert!(sender.send(event("done")).is_err());
}

#[tokio::test]
async fn stalled_writer_has_no_normal_deadline_but_shutdown_interrupts_blocked_write() {
    let (sender, receiver) = outbound_channel();
    let (writer, _reader) = tokio::io::duplex(1);
    let (_images, image_rx) = mpsc::channel(2);
    let (finish, finishing) = oneshot::channel();
    let task = tokio::spawn(run_outbound_writer_with_timeout(
        writer,
        receiver,
        image_rx,
        finishing,
        Duration::from_millis(15),
    ));
    sender.send(delta("run", "s", "x")).unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !task.is_finished(),
        "normal slow readers do not inherit shutdown timeout"
    );
    finish.send(()).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}
