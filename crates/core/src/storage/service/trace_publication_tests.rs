use super::*;
use crate::{ConversationTraceRecorder, ConversationTurnTraceTerminalStatus};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

fn fixture() -> (
    tempfile::TempDir,
    StorageService,
    ConversationTraceRecorder,
    ConversationTraceCommitCursor,
) {
    let directory = tempfile::tempdir().unwrap();
    let service = StorageService::open(&directory.path().join("storage.sqlite")).unwrap();
    service.state.connection().unwrap().execute_batch(
        "INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('chat','Trace test',1,1);
         INSERT INTO messages(id,conversation_id,role,content,status,created_at,position)
         VALUES ('assistant','chat','assistant','','pending',1,0);"
    ).unwrap();
    (
        directory,
        service,
        ConversationTraceRecorder::default(),
        ConversationTraceCommitCursor::default(),
    )
}

fn append(
    service: &StorageService,
    cursor: &mut ConversationTraceCommitCursor,
    recorder: &ConversationTraceRecorder,
) -> Result<ConversationTraceAppendOutcome, String> {
    service.append_trusted_conversation_trace_publication(
        cursor,
        &recorder.publication(),
        "chat",
        "assistant",
        "run",
        1,
        2,
    )
}

#[test]
fn trusted_suffix_reads_no_historical_payload_and_duplicate_is_a_noop() {
    let (_directory, service, mut recorder, mut cursor) = fixture();
    recorder.record_narration("first").unwrap();
    assert!(append(&service, &mut cursor, &recorder).unwrap().changed);
    service
        .state
        .connection()
        .unwrap()
        .authorizer(Some(|context: AuthContext<'_>| {
            if context.accessor.is_none()
                && matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "conversation_turn_trace_items",
                        column_name: "item_json",
                    } | AuthAction::Read {
                        table_name: "conversation_model_context_items",
                        column_name: "payload"
                    }
                )
            {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }));
    recorder.record_narration("second").unwrap();
    let publication = recorder.publication();
    assert!(
        service
            .append_trusted_conversation_trace_publication(
                &mut cursor,
                &publication,
                "chat",
                "assistant",
                "run",
                1,
                2
            )
            .unwrap()
            .changed
    );
    let before = service.state.connection().unwrap().total_changes();
    assert!(
        !service
            .append_trusted_conversation_trace_publication(
                &mut cursor,
                &publication,
                "chat",
                "assistant",
                "run",
                1,
                2
            )
            .unwrap()
            .changed
    );
    assert_eq!(service.state.connection().unwrap().total_changes(), before);
}

#[test]
fn rollback_mid_append_drops_cursor_and_retries_from_the_durable_prefix() {
    let (_directory, service, mut recorder, mut cursor) = fixture();
    recorder.record_narration("first").unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    recorder.record_narration("second").unwrap();
    service.state.connection().unwrap().execute_batch("CREATE TEMP TRIGGER reject_model_insert BEFORE INSERT ON conversation_model_context_items BEGIN SELECT RAISE(ABORT,'injected model write failure'); END;").unwrap();
    assert!(append(&service, &mut cursor, &recorder).is_err());
    assert!(cursor.committed.is_none());
    let connection = service.state.connection().unwrap();
    let trace_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_turn_trace_items",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trace_count, 1);
    connection
        .execute_batch("DROP TRIGGER reject_model_insert")
        .unwrap();
    drop(connection);
    let outcome = append(&service, &mut cursor, &recorder).unwrap();
    assert!(outcome.previous_publication.is_none());
    assert!(outcome.changed);
    let trace = service
        .get_conversation_turn_trace("assistant")
        .unwrap()
        .unwrap();
    assert_eq!(trace.items, recorder.snapshot().items);
}

#[test]
fn tampered_prefix_and_old_cursor_are_fully_revalidated() {
    let (_directory, service, mut recorder, mut cursor) = fixture();
    recorder.record_narration("first").unwrap();
    let first = recorder.publication();
    service
        .append_trusted_conversation_trace_publication(
            &mut cursor,
            &first,
            "chat",
            "assistant",
            "run",
            1,
            2,
        )
        .unwrap();
    recorder.record_narration("second").unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    assert!(service
        .append_trusted_conversation_trace_publication(
            &mut cursor,
            &first,
            "chat",
            "assistant",
            "run",
            1,
            2
        )
        .is_err());
    assert!(cursor.committed.is_none());
    append(&service, &mut cursor, &recorder).unwrap();
    service.state.connection().unwrap().execute("UPDATE conversation_turn_trace_items SET item_json=json_set(item_json,'$.content','tampered') WHERE sequence=0", []).unwrap();
    recorder.record_narration("third").unwrap();
    assert!(append(&service, &mut cursor, &recorder)
        .unwrap_err()
        .contains("exact prefix"));
    assert!(cursor.committed.is_none());
}

#[test]
fn terminal_settlement_and_owner_changes_revoke_append_authority() {
    let (_directory, service, mut recorder, mut cursor) = fixture();
    recorder.record_narration("first").unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    service
        .state
        .connection()
        .unwrap()
        .execute("UPDATE messages SET role='user' WHERE id='assistant'", [])
        .unwrap();
    recorder.record_narration("second").unwrap();
    assert!(append(&service, &mut cursor, &recorder).is_err());
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET role='assistant' WHERE id='assistant'",
            [],
        )
        .unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    let terminal = recorder.finish(
        "run",
        "chat",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    service
        .replace_conversation_turn_trace(&terminal, 1, 3)
        .unwrap();
    recorder.record_narration("late").unwrap();
    assert!(append(&service, &mut cursor, &recorder)
        .unwrap_err()
        .contains("terminal"));
}

#[test]
fn commit_ack_loss_and_reopened_database_rebuild_without_duplicate_items() {
    let (directory, service, mut recorder, mut cursor) = fixture();
    recorder.record_narration("first").unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    // Losing the process-local acknowledgement after COMMIT is equivalent to losing the cursor.
    cursor = ConversationTraceCommitCursor::default();
    assert!(!append(&service, &mut cursor, &recorder).unwrap().changed);
    drop(service);
    let reopened = StorageService::open(&directory.path().join("storage.sqlite")).unwrap();
    recorder.record_narration("second").unwrap();
    let outcome = append(&reopened, &mut cursor, &recorder).unwrap();
    assert!(outcome.previous_publication.is_none());
    assert_eq!(
        reopened
            .get_conversation_turn_trace("assistant")
            .unwrap()
            .unwrap()
            .items
            .len(),
        2
    );
}

#[test]
fn phase_a_restores_each_journal_once_and_phase_b_restores_neither() {
    let (_directory, service, mut recorder, mut cursor) = fixture();
    recorder.record_narration("first").unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    recorder.record_narration("second").unwrap();
    crate::storage::trace_performance_metrics::start();
    append(&service, &mut cursor, &recorder).unwrap();
    let hot = crate::storage::trace_performance_metrics::finish();
    assert_eq!(
        (hot.trace_loads, hot.context_loads, hot.decompressions),
        (0, 0, 0)
    );
    recorder.record_narration("third").unwrap();
    let snapshot = recorder.snapshot();
    crate::storage::trace_performance_metrics::start();
    service
        .append_conversation_trace_with_previous_projection(
            &snapshot.in_progress_audit_trace("run", "chat", "assistant"),
            &snapshot.model_context_items,
            1,
            2,
        )
        .unwrap();
    let phase_a = crate::storage::trace_performance_metrics::finish();
    assert_eq!(
        (
            phase_a.trace_loads,
            phase_a.context_loads,
            phase_a.decompressions
        ),
        (1, 1, 2)
    );
}

#[test]
fn changed_guidance_journal_cannot_bypass_the_original_replay_checks() {
    let (_directory, service, mut recorder, mut cursor) = fixture();
    service.state.connection().unwrap().execute_batch(
        "INSERT INTO agent_run_guidances(guidance_id,client_message_id,run_id,conversation_id,assistant_message_id,content,status,created_at,updated_at)
         VALUES('guide','client','run','chat','assistant','constraint','queued',1,1);",
    ).unwrap();
    let sequence = recorder
        .record_user_guidance("guide", "client", "constraint", &[], &[], 1)
        .unwrap();
    recorder
        .record_model_message(
            sequence,
            0,
            &crate::llm::LlmMessage::text(crate::llm::LlmMessageRole::User, "constraint"),
        )
        .unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    recorder.record_narration("after guidance").unwrap();
    assert!(append(&service, &mut cursor, &recorder)
        .unwrap()
        .previous_publication
        .is_some());
    service.state.connection().unwrap().execute_batch(
        "UPDATE agent_run_guidances SET status='rejected',applied_trace_sequence=NULL,terminal_reason='external change' WHERE guidance_id='guide';",
    ).unwrap();
    recorder.record_narration("must not commit").unwrap();
    assert!(append(&service, &mut cursor, &recorder)
        .unwrap_err()
        .contains("conflicts"));
    assert!(cursor.committed.is_none());
    assert_eq!(
        service
            .get_conversation_turn_trace("assistant")
            .unwrap()
            .unwrap()
            .items
            .len(),
        2
    );
}

#[test]
fn invalid_hot_suffix_consumes_validation_state_and_corrupt_model_forces_cold_failure() {
    let (_directory, service, mut recorder, mut cursor) = fixture();
    recorder.record_narration("first").unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    let valid = recorder.snapshot();
    let call = crate::AgentToolCall {
        id: "call".into(),
        tool: "read_file".into(),
        args: serde_json::json!({"path":"a"}),
        approval_status: crate::AgentApprovalStatus::NotRequired,
        reason: None,
    };
    recorder.record_tool_call(&call).unwrap();
    recorder.record_narration("illegal split").unwrap();
    assert!(append(&service, &mut cursor, &recorder)
        .unwrap_err()
        .contains("split"));
    assert!(cursor.committed.is_none());
    let mut recorder = ConversationTraceRecorder::from_durable_snapshot(valid);
    recorder.record_narration("valid retry").unwrap();
    assert!(append(&service, &mut cursor, &recorder)
        .unwrap()
        .previous_publication
        .is_none());
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE conversation_model_context_items SET payload=X'000102' WHERE sequence=0",
            [],
        )
        .unwrap();
    recorder.record_narration("after corruption").unwrap();
    assert!(append(&service, &mut cursor, &recorder).is_err());
    assert!(cursor.committed.is_none());
}

#[test]
fn concurrent_terminal_commit_never_reopens_a_settled_turn() {
    let (_directory, service, mut recorder, mut cursor) = fixture();
    recorder.record_narration("first").unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    let terminal = recorder.finish(
        "run",
        "chat",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    let publication = recorder.publication();
    let service = Arc::new(service);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    std::thread::scope(|scope| {
        let terminal_service = service.clone();
        let terminal_barrier = barrier.clone();
        let terminal_thread = scope.spawn(move || {
            terminal_barrier.wait();
            terminal_service
                .replace_conversation_turn_trace(&terminal, 1, 3)
                .unwrap();
        });
        barrier.wait();
        // Either a no-op append precedes terminal settlement, or it observes terminal and fails.
        let outcome = service.append_trusted_conversation_trace_publication(
            &mut cursor,
            &publication,
            "chat",
            "assistant",
            "run",
            1,
            2,
        );
        if let Ok(outcome) = outcome {
            assert!(!outcome.changed);
        }
        terminal_thread.join().unwrap();
    });
    assert_eq!(
        service
            .get_conversation_turn_trace("assistant")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    assert!(service
        .append_trusted_conversation_trace_publication(
            &mut cursor,
            &publication,
            "chat",
            "assistant",
            "run",
            1,
            2
        )
        .is_err());
}

#[cfg(unix)]
#[path = "trace_publication_benchmark.rs"]
mod benchmark;
