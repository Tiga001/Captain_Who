//! Bounded, per-segment presentation checkpoints. Trace/model context, pending decisions and
//! terminal status keep their existing transactional owners. Notifications are never write ACKs.
use super::*;
use mycopilot_core::storage::service::{
    CoreRuntimeCheckpoint, CoreRuntimeCheckpointOutcome, CoreRuntimeCheckpointState,
};
use serde_json::{json, Map};
use std::collections::BTreeSet;

// Matches the former ordinary Renderer checkpoint interval. Child observers previously had no
// durable open-stream checkpoint at all. Writer congestion can delay either implementation.
const CHECKPOINT_INTERVAL: Duration = Duration::from_millis(300);

const PROJECTION_FIELDS: &[&str] = &[
    "toolDefinitions",
    "toolSetRevision",
    "todo",
    "firstResponseAt",
    "lastResponseAt",
    "explicitSkillSelections",
    "activatedSkills",
    "skillActivationRevision",
    "state",
    "timeline",
    "messageStreamCheckpoints",
    "fileChanges",
    "skillInstallations",
    "finishReason",
    "error",
    "toolCalls",
    "toolResults",
    "readActivities",
];

struct Projection {
    content: String,
    fields: Map<String, Value>,
    dirty: BTreeSet<String>,
    content_dirty: bool,
    retired: bool,
    sequence: u64,
    // Never discard an unacknowledged request. Even after a lost commit ACK its exact identity
    // and fingerprint must be retried before a later coalesced snapshot can advance the CAS.
    pending: Option<CoreRuntimeCheckpoint>,
}

pub(super) struct RuntimeCheckpoint {
    storage: Arc<StorageService>,
    conversation_id: String,
    assistant_message_id: String,
    run_id: String,
    generation: String,
    projection: Mutex<Projection>,
    flush_gate: Mutex<()>,
}

impl RuntimeCheckpoint {
    pub(super) fn new(
        storage: Arc<StorageService>,
        conversation_id: String,
        assistant_message_id: String,
        run_id: String,
        initial: CoreRuntimeCheckpointState,
        input: &AgentChatInput,
    ) -> Result<Arc<Self>, String> {
        let existing: Value =
            serde_json::from_str(&initial.projection_json).map_err(|error| error.to_string())?;
        let mut fields = Map::new();
        for field in PROJECTION_FIELDS {
            if let Some(value) = existing.get(*field) {
                fields.insert((*field).into(), value.clone());
            }
        }
        // Anchored timeline is reconstructed from Trace. Do not serialize the entire tool loop
        // with every token checkpoint, nor retain it in this coalescer.
        if let Some(items) = fields.get_mut("timeline").and_then(Value::as_array_mut) {
            items.retain(is_open_stream);
        }
        for field in ["toolCalls", "toolResults"] {
            if let Some(items) = fields.get_mut(field).and_then(Value::as_array_mut) {
                let key = if field == "toolCalls" { "id" } else { "callId" };
                items.retain(|item| {
                    item[key].as_str().is_some_and(|id| {
                        initial
                            .presentation_call_ids
                            .iter()
                            .any(|proof| proof == id)
                    })
                });
            }
        }
        // Completed display receipts already belong to SQLite. Keep only unfinished calls for
        // a later result; do not carry an old tool loop into each tick's changed-item patch.
        let completed: HashSet<String> = fields
            .get("toolResults")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item["callId"].as_str().map(str::to_owned))
            .collect();
        for (field, key) in [("toolCalls", "id"), ("readActivities", "callId")] {
            if let Some(items) = fields.get_mut(field).and_then(Value::as_array_mut) {
                items.retain(|item| item[key].as_str().is_some_and(|id| !completed.contains(id)));
            }
        }
        fields.insert("toolResults".into(), json!([]));
        fields.insert("fileChanges".into(), json!([]));
        let mut dirty = BTreeSet::new();
        if !fields.contains_key("explicitSkillSelections")
            && input.resume_checkpoint.is_none()
            && input.tool_continuation.is_none()
            && input.approval_decision.is_none()
        {
            fields.insert(
                "explicitSkillSelections".into(),
                json!(input
                    .skill_activation
                    .as_ref()
                    .map(|activation| activation
                        .skills
                        .iter()
                        .map(|skill| json!({"id":skill.id,"revision":skill.revision}))
                        .collect::<Vec<_>>())
                    .unwrap_or_default()),
            );
            dirty.insert("explicitSkillSelections".into());
        }
        Ok(Arc::new(Self {
            storage,
            conversation_id,
            assistant_message_id,
            run_id,
            generation: initial.generation,
            projection: Mutex::new(Projection {
                content: initial.content,
                fields,
                dirty,
                content_dirty: false,
                retired: false,
                sequence: initial.sequence,
                pending: None,
            }),
            flush_gate: Mutex::new(()),
        }))
    }

    pub(super) fn record(&self, event: &AgentEvent) {
        // Avoid serializing ignored events (in particular private tool input/output previews).
        if !matches!(
            event,
            AgentEvent::Started { .. }
                | AgentEvent::ToolSetChanged { .. }
                | AgentEvent::State { .. }
                | AgentEvent::MessageDelta { .. }
                | AgentEvent::MessageStreamStarted { .. }
                | AgentEvent::MessageStreamReset { .. }
                | AgentEvent::MessageStreamCommitted { .. }
                | AgentEvent::Message { .. }
                | AgentEvent::TodoUpdated { .. }
                | AgentEvent::FileChangeUpdated { .. }
                | AgentEvent::Error { .. }
                | AgentEvent::Done { .. }
                | AgentEvent::ApprovalRequired { .. }
                | AgentEvent::ToolCall { .. }
                | AgentEvent::ToolResult { .. }
        ) {
            return;
        }
        let safe = RendererSafeAgentEvent::new(event).into_projection();
        let mut projection = self
            .projection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if projection.retired {
            return;
        }
        projection.apply(&safe, mycopilot_core::storage::now_ms());
    }

    pub(super) fn set_waiting_finish_reason(&self, reason: Option<&str>) {
        if let Some(reason) = reason {
            let mut projection = self.projection.lock().unwrap_or_else(|e| e.into_inner());
            if !projection.retired {
                projection.set("finishReason", reason.into());
            }
        }
    }

    pub(super) fn content(&self) -> String {
        self.projection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .content
            .clone()
    }

    /// The durable pending/suspension transaction now owns the handoff. Later State/Done
    /// events from this retiring segment must not add a second fallible projection barrier.
    pub(super) fn seal_waiting(&self) {
        self.projection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retired = true;
    }

    /// The only synchronous barriers are at already-synchronous pending-action boundaries.
    /// The gate is private to this Runtime segment; no global registry is held during SQLite.
    pub(super) fn flush(&self) -> Result<(), String> {
        let _flush = self
            .flush_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // At most an unacknowledged request plus the latest coalesced state, never an event queue.
        for _ in 0..2 {
            let request = {
                let mut projection = self
                    .projection
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if projection.retired {
                    return Ok(());
                }
                if projection.pending.is_none() {
                    if projection.dirty.is_empty() && !projection.content_dirty {
                        return Ok(());
                    }
                    let dirty = std::mem::take(&mut projection.dirty);
                    let patch: Map<String, Value> = dirty
                        .into_iter()
                        .filter_map(|key| {
                            projection
                                .fields
                                .get(&key)
                                .cloned()
                                .map(|value| (key, value))
                        })
                        .collect();
                    let projection_json =
                        serde_json::to_string(&patch).map_err(|e| e.to_string())?;
                    projection.pending = Some(CoreRuntimeCheckpoint {
                        conversation_id: self.conversation_id.clone(),
                        assistant_message_id: self.assistant_message_id.clone(),
                        run_id: self.run_id.clone(),
                        generation: self.generation.clone(),
                        expected_sequence: projection.sequence,
                        sequence: projection.sequence + 1,
                        content: projection.content.clone(),
                        projection_json,
                    });
                    projection.content_dirty = false;
                }
                projection.pending.as_ref().unwrap().clone()
            };
            let outcome = self.storage.apply_runtime_checkpoint(&request)?;
            let mut projection = self
                .projection
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            projection.pending = None;
            match outcome {
                CoreRuntimeCheckpointOutcome::Persisted
                | CoreRuntimeCheckpointOutcome::Replayed => {
                    projection.prune_confirmed_items(&request.projection_json);
                    projection.sequence = request.sequence;
                }
                CoreRuntimeCheckpointOutcome::Retired => projection.retired = true,
            }
        }
        Ok(())
    }

    fn estimated_bytes(&self) -> usize {
        let state = self.projection.lock().unwrap_or_else(|e| e.into_inner());
        if state.retired
            || (state.dirty.is_empty() && !state.content_dirty && state.pending.is_none())
        {
            return 0;
        }
        fn size(value: &Value) -> usize {
            struct Size(usize);
            impl std::io::Write for Size {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    self.0 = self.0.saturating_add(bytes.len());
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let mut measured = Size(0);
            serde_json::to_writer(&mut measured, value)
                .expect("JSON Value serialization to counter is infallible");
            measured.0
        }
        // Only owned checkpoint payloads, not the immutable Trace or conversation history.
        let latest = state.content.len().saturating_add(
            state
                .dirty
                .iter()
                .filter_map(|key| state.fields.get(key))
                .map(size)
                .sum::<usize>(),
        );
        // The worker copies and submits one request at a time. The latest state and old pending
        // request are already owned by the Run, not duplicated in the executor's queue.
        latest
            .max(
                state
                    .pending
                    .as_ref()
                    .map_or(0, |p| p.content.len() + p.projection_json.len()),
            )
            .saturating_add(4096)
    }

    pub(super) async fn flush_async(
        self: &Arc<Self>,
        executor: &crate::application::blocking_executor::BlockingExecutor,
    ) -> Result<(), String> {
        let bytes = self.estimated_bytes();
        if bytes == 0 {
            return Ok(());
        }
        let reservation = executor.reserve(bytes).await.map_err(|e| e.to_string())?;
        let checkpoint = self.clone();
        reservation
            .submit(move || checkpoint.flush())
            .wait()
            .await
            .map_err(|e| e.to_string())?
    }

    pub(super) fn start_timer(
        self: &Arc<Self>,
        executor: crate::application::blocking_executor::BlockingExecutor,
    ) -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let checkpoint = self.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            tokio::pin!(stopped);
            let mut timer = tokio::time::interval_at(
                tokio::time::Instant::now() + CHECKPOINT_INTERVAL,
                CHECKPOINT_INTERVAL,
            );
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    _ = timer.tick() => {
                        if let Err(error) = checkpoint.flush_async(&executor).await {
                            eprintln!("Runtime checkpoint retained for retry ({}): {error}", checkpoint.run_id);
                        }
                    }
                }
            }
        });
        (stop, task)
    }
}

fn is_open_stream(item: &Value) -> bool {
    item["type"] == "message" && item["streamId"].is_string() && !item["traceSequence"].is_u64()
}

impl Projection {
    fn prune_confirmed_items(&mut self, patch: &str) {
        let saved: Value =
            serde_json::from_str(patch).expect("internally encoded checkpoint patch");
        let completed: HashSet<String> = saved["toolResults"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| {
                self.fields
                    .get("toolResults")
                    .and_then(Value::as_array)
                    .is_some_and(|current| current.contains(item))
            })
            .filter_map(|item| item["callId"].as_str().map(str::to_owned))
            .collect();
        for (field, key) in [
            ("toolCalls", "id"),
            ("toolResults", "callId"),
            ("readActivities", "callId"),
        ] {
            if let Some(items) = self.fields.get_mut(field).and_then(Value::as_array_mut) {
                items.retain(|item| !item[key].as_str().is_some_and(|id| completed.contains(id)));
            }
        }
        if let Some(items) = self
            .fields
            .get_mut("fileChanges")
            .and_then(Value::as_array_mut)
        {
            items.retain(|item| {
                !saved["fileChanges"]
                    .as_array()
                    .is_some_and(|saved| saved.contains(item))
            });
        }
    }
    fn set(&mut self, key: &str, value: Value) {
        if self.fields.get(key) != Some(&value) {
            self.fields.insert(key.into(), value);
            self.dirty.insert(key.into());
        }
    }
    fn response_time(&mut self, now: i64) {
        if !self.fields.contains_key("firstResponseAt") {
            self.set("firstResponseAt", now.into());
        }
        self.set("lastResponseAt", now.into());
    }
    fn apply(&mut self, event: &Value, now: i64) {
        for field in
            super::runtime_checkpoint_web_read::project_web_read(&mut self.fields, event, now)
        {
            self.dirty.insert(field.into());
        }
        if super::runtime_checkpoint_activities::project_checkpoint_activity(
            &mut self.fields,
            event,
            now,
        ) {
            self.dirty.insert("skillInstallations".into());
        }
        let stream = event["streamId"].as_str().unwrap_or("legacy");
        match event["type"].as_str().unwrap_or_default() {
            "started" => self.set("toolDefinitions", event["toolDefinitions"].clone()),
            "tool_set_changed" => {
                self.set("toolDefinitions", event["toolDefinitions"].clone());
                self.set(
                    "toolSetRevision",
                    json!({"stable":event["stableRevision"],
                    "dynamic":event["dynamicRevision"],"effective":event["effectiveRevision"]}),
                );
            }
            "state" => {
                if !matches!(
                    event["state"]["status"].as_str(),
                    Some("completed" | "failed" | "cancelled")
                ) {
                    self.set("state", event["state"].clone());
                }
            }
            "todo_updated" => self.set("todo", event["todo"].clone()),
            "message_stream_started" => {
                let checkpoints = self
                    .fields
                    .entry("messageStreamCheckpoints")
                    .or_insert_with(|| json!({}));
                if checkpoints.get(stream).is_some() {
                    return;
                }
                checkpoints[stream] = json!({"previousContent":self.content});
                self.dirty.insert("messageStreamCheckpoints".into());
                self.content.clear();
                self.content_dirty = true;
            }
            "message_stream_reset" => {
                if let Some(checkpoint) = self
                    .fields
                    .get_mut("messageStreamCheckpoints")
                    .and_then(Value::as_object_mut)
                    .and_then(|map| map.remove(stream))
                {
                    self.content = checkpoint["previousContent"]
                        .as_str()
                        .unwrap_or_default()
                        .into();
                    self.content_dirty = true;
                    self.dirty.insert("messageStreamCheckpoints".into());
                    self.remove_stream(stream);
                }
            }
            "message_stream_committed" => {
                if let Some(map) = self
                    .fields
                    .get_mut("messageStreamCheckpoints")
                    .and_then(Value::as_object_mut)
                {
                    map.remove(stream);
                    self.dirty.insert("messageStreamCheckpoints".into());
                }
                if event["traceSequence"].is_u64() {
                    self.remove_stream(stream);
                }
            }
            "message_delta" => {
                let delta = event["delta"].as_str().unwrap_or_default();
                self.content.push_str(delta);
                self.content_dirty = true;
                self.response_time(now);
                let timeline = self
                    .fields
                    .entry("timeline")
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap();
                if let Some(item) = timeline.iter_mut().find(|item| item["streamId"] == stream) {
                    if let Some(Value::String(content)) = item.get_mut("content") {
                        content.push_str(delta);
                    }
                } else {
                    timeline.push(json!({"id":format!("stream:{stream}"),"type":"message",
                        "streamId":stream,"content":delta}));
                }
                self.dirty.insert("timeline".into());
            }
            "message" => {
                self.content = event["content"].as_str().unwrap_or_default().into();
                self.content_dirty = true;
                self.response_time(now);
                self.set(
                    "timeline",
                    json!([{"id":"stream:legacy","type":"message",
                    "streamId":"legacy","content":self.content}]),
                );
            }
            "file_change_updated" => {
                let snapshot = &event["fileChange"];
                let items = self
                    .fields
                    .entry("fileChanges")
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap();
                if let Some(item) = items
                    .iter_mut()
                    .find(|item| item["transactionId"] == snapshot["transactionId"])
                {
                    *item = snapshot.clone();
                } else {
                    items.push(snapshot.clone());
                }
                self.dirty.insert("fileChanges".into());
            }
            "error" if event["recoverable"] == false => self.set("error", event["message"].clone()),
            "done" => {
                if let Some(reason) = event.get("finishReason") {
                    self.set("finishReason", reason.clone());
                }
                // Content, usage and terminal status are committed by the existing terminal
                // transaction. A checkpoint may never announce a successful terminal outcome.
            }
            _ => {}
        }
    }
    fn remove_stream(&mut self, stream: &str) {
        if let Some(items) = self
            .fields
            .get_mut("timeline")
            .and_then(Value::as_array_mut)
        {
            items.retain(|item| item["streamId"] != stream);
            self.dirty.insert("timeline".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn projection() -> Projection {
        Projection {
            content: "previous narration".into(),
            fields: Map::new(),
            dirty: BTreeSet::new(),
            content_dirty: false,
            retired: false,
            sequence: 0,
            pending: None,
        }
    }
    #[test]
    fn reset_restores_exact_content_and_committed_narration_belongs_to_trace() {
        let mut p = projection();
        p.apply(
            &json!({"type":"message_stream_started","streamId":"one"}),
            1,
        );
        p.apply(
            &json!({"type":"message_delta","streamId":"one","delta":"部分 🐈"}),
            2,
        );
        p.apply(&json!({"type":"message_stream_reset","streamId":"one"}), 3);
        assert_eq!(p.content, "previous narration");
        assert_eq!(p.fields["timeline"], json!([]));
        p.apply(
            &json!({"type":"message_stream_started","streamId":"two"}),
            4,
        );
        p.apply(
            &json!({"type":"message_delta","streamId":"two","delta":"narration"}),
            5,
        );
        p.apply(
            &json!({"type":"message_stream_committed","streamId":"two","traceSequence":2}),
            6,
        );
        assert_eq!(p.fields["timeline"], json!([]));
        p.apply(
            &json!({"type":"message_stream_started","streamId":"answer"}),
            7,
        );
        p.apply(
            &json!({"type":"message_delta","streamId":"answer","delta":"final"}),
            8,
        );
        p.apply(
            &json!({"type":"message_stream_committed","streamId":"answer"}),
            9,
        );
        assert_eq!(p.content, "final");
        assert_eq!(p.fields["timeline"][0]["content"], "final");
        assert_eq!(p.fields["messageStreamCheckpoints"], json!({}));
    }
    #[test]
    fn terminal_event_does_not_claim_a_durable_outcome() {
        let mut p = projection();
        p.apply(&json!({"type":"state","state":{"status":"completed"}}), 1);
        p.apply(&json!({"type":"done","status":"completed","content":"too early","finishReason":"stop"}),2);
        assert!(!p.fields.contains_key("status"));
        assert!(!p.fields.contains_key("state"));
        assert_eq!(p.content, "previous narration");
        assert_eq!(p.fields["finishReason"], "stop");
    }

    #[test]
    fn failed_write_and_lost_ack_keep_exact_request_before_coalesced_successor() {
        use mycopilot_core::storage::models::{ChatConversationRecord, ChatMessageRecord};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("coalescer.sqlite");
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &path,
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation".into(),
                project_id: None,
                model_id: None,
                title: "Synthetic".into(),
                messages: vec![ChatMessageRecord {
                    id: "assistant".into(),
                    role: "assistant".into(),
                    content: String::new(),
                    created_at: 1,
                    status: Some("pending".into()),
                    attachments: vec![],
                    agent_run_json: None,
                    folder_references_json: None,
                    ui_state_json: None,
                    human_interaction_response: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let connection = rusqlite::Connection::open(path).unwrap();
        connection.execute("INSERT INTO conversation_turn_traces
            (assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at)
            VALUES ('assistant','conversation','run',?1,'in_progress',0,1,1)",
            [mycopilot_core::CONVERSATION_TURN_TRACE_SCHEMA_VERSION]).unwrap();
        let initial = storage
            .begin_runtime_checkpoint("conversation", "assistant", "run")
            .unwrap()
            .unwrap();
        let checkpoint = RuntimeCheckpoint {
            storage: storage.clone(),
            conversation_id: "conversation".into(),
            assistant_message_id: "assistant".into(),
            run_id: "run".into(),
            generation: initial.generation,
            projection: Mutex::new(Projection {
                content: String::new(),
                ..projection()
            }),
            flush_gate: Mutex::new(()),
        };
        checkpoint
            .projection
            .lock()
            .unwrap()
            .apply(&json!({"type":"message_delta","delta":"first"}), 2);
        connection
            .execute_batch(
                "CREATE TRIGGER failure BEFORE UPDATE OF agent_run_json ON messages
            WHEN json_extract(NEW.agent_run_json,'$.coreCheckpoint.sequence')=1
            BEGIN SELECT RAISE(ABORT,'synthetic checkpoint write failure'); END;",
            )
            .unwrap();
        assert!(checkpoint
            .flush()
            .unwrap_err()
            .contains("synthetic checkpoint write failure"));
        let unacknowledged = checkpoint
            .projection
            .lock()
            .unwrap()
            .pending
            .clone()
            .unwrap();
        assert_eq!(unacknowledged.sequence, 1);
        connection.execute_batch("DROP TRIGGER failure").unwrap();
        // The database commits, but its ACK never reaches this coalescer.
        assert_eq!(
            storage.apply_runtime_checkpoint(&unacknowledged).unwrap(),
            CoreRuntimeCheckpointOutcome::Persisted
        );
        checkpoint
            .projection
            .lock()
            .unwrap()
            .apply(&json!({"type":"message_delta","delta":" + latest"}), 3);
        checkpoint.flush().unwrap();
        assert!(checkpoint.projection.lock().unwrap().pending.is_none());
        assert_eq!(checkpoint.projection.lock().unwrap().sequence, 2);
        let (content, raw): (String, String) = connection
            .query_row(
                "SELECT content,agent_run_json FROM messages WHERE id='assistant'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(content, "first + latest");
        let run: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(run["timeline"][0]["content"], "first + latest");
        assert_eq!(checkpoint.estimated_bytes(), 0);
    }
}
