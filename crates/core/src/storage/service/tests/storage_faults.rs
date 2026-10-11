//! Isolated disk fixtures. SQLITE_FULL below is SQLite's page limit, not an OS ENOSPC/fsync test.
use super::*;
use crate::storage::{StorageEventStream, StorageWriteClass};
use crate::ConversationTurnTraceTerminalStatus;
use rusqlite::types::Value as SqlValue;
use std::time::{Duration, Instant};

const CHAT: &str = "fault-chat";
const MESSAGE: &str = "fault-assistant";
const RUN: &str = "run-1";
const CONFIRMED: &str = "confirmed checkpoint 🐈";
const TERMINAL: &str = "committed terminal 🐈";

fn open(path: &Path) -> StorageService {
    StorageService::open_with_model_credentials(
        path,
        Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap()
}

fn seed(service: &StorageService) -> CoreRuntimeCheckpoint {
    let mut record = conversation(CHAT, None, "fault-user");
    record.messages.push(ChatMessageRecord {
        id: MESSAGE.into(),
        role: "assistant".into(),
        content: String::new(),
        created_at: 2,
        status: Some("pending".into()),
        attachments: Vec::new(),
        agent_run_json: Some(
            chat_repository::canonical_agent_run_lifecycle_projection(
                None, RUN, "running", 2, 2, None,
            )
            .unwrap(),
        ),
        folder_references_json: None,
        ui_state_json: None,
        human_interaction_response: None,
    });
    service.save_conversation(record).unwrap();
    service
        .append_in_progress_conversation_turn_trace(
            &crate::ConversationTraceSnapshot::default().in_progress_trace(RUN, CHAT, MESSAGE),
            2,
            2,
        )
        .unwrap();
    let initial = service
        .begin_runtime_checkpoint(CHAT, MESSAGE, RUN)
        .unwrap()
        .unwrap();
    let checkpoint = CoreRuntimeCheckpoint {
        conversation_id: CHAT.into(),
        assistant_message_id: MESSAGE.into(),
        run_id: RUN.into(),
        generation: initial.generation,
        expected_sequence: 0,
        sequence: 1,
        content: CONFIRMED.into(),
        projection_json: serde_json::json!({"firstResponseAt":3,"lastResponseAt":3}).to_string(),
    };
    assert_eq!(
        service.apply_runtime_checkpoint(&checkpoint).unwrap(),
        CoreRuntimeCheckpointOutcome::Persisted
    );
    checkpoint
}

fn finalize(service: &StorageService) -> Result<(), String> {
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: RUN.into(),
        conversation_id: CHAT.into(),
        assistant_message_id: MESSAGE.into(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            provider_turn_id: None,
            first_tool_call_id: None,
            sequence: 0,
            content: TERMINAL.into(),
            truncated: false,
        }],
    };
    let context = [ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".into(),
        content: TERMINAL.into(),
        images: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        is_error: false,
    }];
    let mut usage = agent_usage_record(CHAT, MESSAGE);
    usage.project_id = None;
    service.finalize_chat_message_with_conversation_turn_notification(
        CHAT,
        MESSAGE,
        TERMINAL,
        Some("sent"),
        "completed",
        &trace,
        Some(&context),
        2,
        1_000,
        Some(&usage),
        &notification_repository::NewNotificationEventRecord {
            notification_kind: "task_completed".into(),
            source_kind: "human_root".into(),
            source_id: RUN.into(),
            run_id: Some(RUN.into()),
            automation_id: None,
            conversation_id: Some(CHAT.into()),
            user_message_id: Some("fault-user".into()),
            assistant_message_id: Some(MESSAGE.into()),
            approval_action_id: None,
            subject_kind: "prompt_excerpt".into(),
            subject_text: "Synthetic terminal".into(),
            dedupe_key: "fault-terminal".into(),
            supersession_key: "fault-run".into(),
            resource_revision: Some(1),
            occurred_at: 1_000,
            expires_at: i64::MAX,
        },
        None,
    )
}

fn rows(connection: &rusqlite::Connection, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut statement = connection.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| (0..columns).map(|index| row.get(index)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn durable_facts(service: &StorageService) -> Vec<Vec<Vec<SqlValue>>> {
    let connection = service.state.read_connection().unwrap();
    let snapshot = connection.unchecked_transaction().unwrap();
    [
        "SELECT * FROM messages ORDER BY id",
        "SELECT * FROM conversation_turn_traces ORDER BY assistant_message_id",
        "SELECT * FROM conversation_turn_trace_items ORDER BY assistant_message_id, sequence",
        "SELECT * FROM conversation_model_context_items ORDER BY assistant_message_id, sequence, ordinal",
        "SELECT * FROM agent_usage_records ORDER BY id",
        "SELECT * FROM agent_pending_actions ORDER BY action_id",
        "SELECT * FROM notification_events ORDER BY id",
        "SELECT * FROM notification_batches ORDER BY id",
        "SELECT * FROM notification_batch_items ORDER BY batch_id, notification_event_id",
        "SELECT * FROM notification_change_events ORDER BY sequence",
    ]
    .iter()
    .map(|sql| rows(&snapshot, sql))
    .collect()
}

fn assert_terminal_state(service: &StorageService, completed: bool) {
    let connection = service.state.read_connection().unwrap();
    let (content, status, raw): (String, String, String) = connection
        .query_row(
            "SELECT content,status,agent_run_json FROM messages WHERE id=?1",
            [MESSAGE],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let run: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(content, if completed { TERMINAL } else { CONFIRMED });
    assert_eq!(status, if completed { "sent" } else { "pending" });
    assert_eq!(
        run["status"],
        if completed { "completed" } else { "running" }
    );
    assert_eq!(run["coreCheckpoint"]["sequence"], 1);
    let path = connection.path().expect("isolated disk fixture").to_owned();
    drop(connection);
    // The isolated reproduction records a connection-sensitive FTS5 quick_check mismatch after
    // another connection commits (also on SQLite 3.51.0). Check the durable index with a fresh
    // read-only handle; this does not rebuild FTS or assume an unconfirmed internal root cause.
    let integrity = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    assert_eq!(
        rows(&integrity, "PRAGMA quick_check"),
        vec![vec![SqlValue::Text("ok".into())]]
    );
    assert!(rows(&integrity, "PRAGMA foreign_key_check").is_empty());
    drop(integrity);
    let trace = service
        .get_conversation_turn_trace(MESSAGE)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        if completed {
            ConversationTurnTraceTerminalStatus::Completed
        } else {
            ConversationTurnTraceTerminalStatus::InProgress
        }
    );
    assert_eq!(trace.items.len(), usize::from(completed));
    let context = service.get_conversation_model_context_log(MESSAGE).unwrap();
    assert_eq!(
        context.map_or(0, |log| log.items.len()),
        usize::from(completed)
    );
    let connection = service.state.read_connection().unwrap();
    for table in ["agent_usage_records", "notification_events"] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            i64::from(completed)
        );
    }
}

#[test]
fn sqlite_full_at_terminal_notification_rolls_back_all_facts_and_retries() {
    let directory = tempfile::tempdir().unwrap();
    let service = open(&directory.path().join("full.sqlite"));
    let checkpoint = seed(&service);
    let before = durable_facts(&service);
    let notifications = service
        .state
        .event_notifications()
        .subscribe(StorageEventStream::Notification);
    let original_limit = service
        .state
        .write_sync(1, StorageWriteClass::Control, |connection| {
            connection
                .execute_batch(
                    "CREATE TABLE fault_capacity(payload BLOB);
                 CREATE TRIGGER fault_full_at_notification BEFORE INSERT ON notification_events
                 BEGIN INSERT INTO fault_capacity VALUES(zeroblob(4194304)); END;",
                )
                .unwrap();
            let original: i64 = connection
                .query_row("PRAGMA max_page_count", [], |r| r.get(0))
                .unwrap();
            let pages: i64 = connection
                .query_row("PRAGMA page_count", [], |r| r.get(0))
                .unwrap();
            // Plenty for the small terminal facts, insufficient for the tail trigger's 4 MiB.
            connection
                .pragma_update(None, "max_page_count", pages + 128)
                .unwrap();
            original
        })
        .unwrap();
    let error = finalize(&service).unwrap_err();
    assert!(error.contains("database or disk is full"), "{error}");
    assert_eq!(durable_facts(&service), before);
    assert!(!notifications.has_changed().unwrap());
    assert_terminal_state(&service, false);
    service
        .state
        .write_sync(1, StorageWriteClass::Control, |connection| {
            assert!(connection.is_autocommit());
            connection
                .pragma_update(None, "max_page_count", original_limit)
                .unwrap();
            connection
                .execute_batch(
                    "DROP TRIGGER fault_full_at_notification; DROP TABLE fault_capacity;",
                )
                .unwrap();
        })
        .unwrap();
    assert_eq!(
        service.apply_runtime_checkpoint(&checkpoint).unwrap(),
        CoreRuntimeCheckpointOutcome::Replayed
    );
    finalize(&service).unwrap();
    assert_terminal_state(&service, true);
    assert!(notifications.has_changed().unwrap());
    assert_eq!(
        service.apply_runtime_checkpoint(&checkpoint).unwrap(),
        CoreRuntimeCheckpointOutcome::Retired
    );
}

#[test]
fn sqlite_backup_restores_checkpoint_terminal_pending_and_notification_cursor_together() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite");
    let backup = directory.path().join("backup.sqlite");
    let service = open(&source);
    let checkpoint = seed(&service);
    // An independent SQLite reader pins pre-terminal pages in WAL throughout the backup.
    let mut reader = rusqlite::Connection::open(&source).unwrap();
    let pinned = reader.transaction().unwrap();
    assert_eq!(
        rows(
            &pinned,
            "SELECT terminal_status FROM conversation_turn_traces"
        ),
        vec![vec![SqlValue::Text("in_progress".into())]]
    );
    finalize(&service).unwrap();
    let mut waiting = conversation("waiting-chat", None, "waiting-user");
    waiting.messages[0].content = "Pending input remains available".into();
    service.save_conversation(waiting).unwrap();
    let mut pending = pending_action("waiting-action", "waiting-chat");
    pending.run_id = "waiting-run".into();
    pending.assistant_message_id = None;
    service.store_pending_agent_action(pending.clone()).unwrap();
    let facts = durable_facts(&service);
    let cursor = service.latest_notification_change_sequence().unwrap();
    assert!(cursor > 0);
    std::fs::File::create(&backup).unwrap();
    crate::storage::create_verified_sqlite_snapshot(&source, &backup).unwrap();
    let restored = open(&backup);
    assert_eq!(durable_facts(&restored), facts);
    assert_terminal_state(&restored, true);
    assert_eq!(
        restored.get_pending_agent_action("waiting-action").unwrap(),
        Some(pending)
    );
    assert_eq!(
        restored.latest_notification_change_sequence().unwrap(),
        cursor
    );
    assert!(restored
        .list_notification_change_events_after(cursor, 100)
        .unwrap()
        .is_empty());
    assert_eq!(
        restored.apply_runtime_checkpoint(&checkpoint).unwrap(),
        CoreRuntimeCheckpointOutcome::Retired
    );
    let delivered = restored
        .claim_pending_notification_batches("restored-claim", 5_000, 10)
        .unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(
        restored
            .mark_notification_events_seen(None, Some(&delivered[0].id), false, now_ms())
            .unwrap(),
        1
    );
    assert!(restored.latest_notification_change_sequence().unwrap() > cursor);
    // Recovery never modifies the original WAL or its independently pinned old snapshot.
    assert_eq!(durable_facts(&service), facts);
    assert_eq!(
        rows(
            &pinned,
            "SELECT terminal_status FROM conversation_turn_traces"
        ),
        vec![vec![SqlValue::Text("in_progress".into())]]
    );
}

#[test]
fn terminal_crash_fixture() {
    let Ok(directory) = std::env::var("CAPTAIN_STORAGE_TERMINAL_CRASH_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(directory);
    let service = open(&root.join("crash.sqlite"));
    seed(&service);
    let ready = root.join("ready");
    if std::env::var("CAPTAIN_STORAGE_TERMINAL_CRASH_PHASE").unwrap() == "before-commit" {
        service
            .state
            .write_sync(1, StorageWriteClass::Control, move |connection| {
                connection
                    .commit_hook(Some(move || {
                        std::fs::write(ready.with_extension("staging"), b"before-commit").unwrap();
                        std::fs::rename(ready.with_extension("staging"), &ready).unwrap();
                        loop {
                            std::thread::park();
                        }
                    }))
                    .unwrap();
            })
            .unwrap();
    }
    finalize(&service).unwrap();
    std::fs::write(root.join("ready.staging"), b"after-commit").unwrap();
    std::fs::rename(root.join("ready.staging"), root.join("ready")).unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn process_kill_at_terminal_commit_boundary_restores_an_atomic_prefix() {
    for phase in ["before-commit", "after-commit"] {
        let directory = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::service::tests::storage_faults::terminal_crash_fixture",
                "--nocapture",
            ])
            .env("CAPTAIN_STORAGE_TERMINAL_CRASH_FIXTURE", directory.path())
            .env("CAPTAIN_STORAGE_TERMINAL_CRASH_PHASE", phase)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let ready = directory.path().join("ready");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ready.exists() && Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let observed = std::fs::read_to_string(&ready).ok();
        let _ = child.kill();
        let result = child.wait_with_output().unwrap();
        assert_eq!(
            observed.as_deref(),
            Some(phase),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!result.status.success());
        let restored = open(&directory.path().join("crash.sqlite"));
        assert_terminal_state(&restored, phase == "after-commit");
        if phase == "before-commit" {
            finalize(&restored).unwrap();
            assert_terminal_state(&restored, true);
        }
    }
}
