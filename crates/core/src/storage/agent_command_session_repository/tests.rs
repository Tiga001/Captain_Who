use super::*;
use crate::storage::conversation_history_archive_repository::{
    self, ConversationHistoryArchiveInput,
};
use crate::storage::{chat_repository, migrations};
use serde_json::json;

const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn test_connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    connection
}

fn seed_conversation(
    connection: &Connection,
    project_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) {
    connection
        .execute(
            "INSERT OR IGNORE INTO projects (
                 id, name, path, created_at, pinned_at, updated_at
             ) VALUES (?1, ?1, '/tmp/project', 1, NULL, 1)",
            [project_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO conversations (
                 id, project_id, title, created_at, updated_at
             ) VALUES (?1, ?2, ?1, 1, 1)",
            params![conversation_id, project_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO messages (
                 id, conversation_id, role, content, status, created_at, position
             ) VALUES (?1, ?2, 'assistant', '', 'streaming', 1, 0)",
            params![assistant_message_id, conversation_id],
        )
        .unwrap();
}

fn create_input(
    session_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    project_id: &str,
    started_at: u64,
) -> AgentCommandSessionCreate {
    AgentCommandSessionCreate {
        snapshot: AgentCommandSessionSnapshot {
            schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            origin_run_id: format!("run-{session_id}"),
            call_id: format!("call-{session_id}"),
            project_id: Some(project_id.to_string()),
            command: "python3 app.py".to_string(),
            cwd: "/tmp/project".to_string(),
            command_digest: DIGEST.to_string(),
            status: AgentCommandSessionStatus::Starting,
            started_at,
            ended_at: None,
            exit_code: None,
            latest_sequence: 0,
            output_truncated: false,
            archive_ref: None,
        },
        authorization_source: CommandAuthorizationSource::ExplicitUser,
        approval_provenance: json!({"actionId": "action-1", "decision": "approved"}),
        permission_provenance: json!({"mode": "default"}),
        created_at: i64::try_from(started_at).unwrap(),
    }
}

#[test]
fn create_get_and_list_are_idempotent_and_conversation_scoped() {
    let mut connection = test_connection();
    seed_conversation(&connection, "project-1", "conversation-1", "assistant-1");
    seed_conversation(&connection, "project-2", "conversation-2", "assistant-2");
    let input = create_input(
        "cmd_00000000000000000000000000000001",
        "conversation-1",
        "assistant-1",
        "project-1",
        10,
    );

    assert_eq!(
        create_session(&mut connection, &input).unwrap(),
        AgentCommandSessionCreateOutcome::Inserted
    );
    assert_eq!(
        create_session(&mut connection, &input).unwrap(),
        AgentCommandSessionCreateOutcome::Idempotent
    );
    assert!(get_session(
        &connection,
        "conversation-2",
        "cmd_00000000000000000000000000000001"
    )
    .unwrap()
    .is_none());
    let listed = list_sessions_for_conversation(&connection, "conversation-1", 8).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].snapshot.conversation_id, "conversation-1");
    assert_eq!(
        listed[0].authorization_source,
        CommandAuthorizationSource::ExplicitUser
    );
}

#[test]
fn output_and_model_cursors_are_independent_and_terminal_cas_is_idempotent() {
    let mut connection = test_connection();
    seed_conversation(&connection, "project-1", "conversation-1", "assistant-1");
    let session_id = "cmd_00000000000000000000000000000002";
    create_session(
        &mut connection,
        &create_input(session_id, "conversation-1", "assistant-1", "project-1", 10),
    )
    .unwrap();
    assert_eq!(
        mark_running(&mut connection, "conversation-1", session_id, 11).unwrap(),
        AgentCommandSessionTransitionOutcome::Updated
    );
    let chunks = vec![
        AgentCommandSessionOutputChunk {
            sequence: 1,
            stream: AgentCommandOutputStream::Stdout,
            output: "one".to_string(),
        },
        AgentCommandSessionOutputChunk {
            sequence: 2,
            stream: AgentCommandOutputStream::Stderr,
            output: "two".to_string(),
        },
    ];
    let append = AgentCommandSessionOutputAppend {
        conversation_id: "conversation-1",
        session_id,
        chunks: &chunks,
        latest_sequence: 2,
        transcript_truncated: false,
        output_capture_truncated: false,
        updated_at: 12,
    };
    assert_eq!(
        append_output(&mut connection, &append).unwrap(),
        AgentCommandSessionTransitionOutcome::Updated
    );
    assert_eq!(
        append_output(&mut connection, &append).unwrap(),
        AgentCommandSessionTransitionOutcome::Idempotent
    );

    let renderer = read_transcript(&connection, "conversation-1", session_id, 0, 1024)
        .unwrap()
        .unwrap();
    assert_eq!(renderer.chunks, chunks);
    assert!(
        advance_model_read_sequence(&connection, "conversation-1", session_id, 0, 2, 13,).unwrap()
    );
    assert!(
        !advance_model_read_sequence(&connection, "conversation-1", session_id, 0, 2, 14,).unwrap()
    );
    let renderer_again = read_transcript(&connection, "conversation-1", session_id, 0, 1024)
        .unwrap()
        .unwrap();
    assert_eq!(renderer_again.chunks, chunks);

    let terminal = AgentCommandSessionTerminalUpdate {
        conversation_id: "conversation-1",
        session_id,
        status: AgentCommandSessionStatus::Exited,
        ended_at: 20,
        exit_code: Some(0),
        latest_sequence: 2,
        transcript_truncated: false,
        output_capture_truncated: false,
        archive_ref: None,
        terminal_reason: None,
        committed_at: 20,
    };
    assert_eq!(
        commit_terminal(&mut connection, &terminal).unwrap(),
        AgentCommandSessionTransitionOutcome::Updated
    );
    assert_eq!(
        commit_terminal(&mut connection, &terminal).unwrap(),
        AgentCommandSessionTransitionOutcome::Idempotent
    );
    assert!(matches!(
        append_output(&mut connection, &append).unwrap(),
        AgentCommandSessionTransitionOutcome::Conflict {
            current_status: AgentCommandSessionStatus::Exited
        }
    ));
}

#[test]
fn model_read_receipt_replays_the_same_cut_after_reopen_and_is_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("storage.sqlite");
    let session_id = "cmd_00000000000000000000000000000008";
    let mut connection = Connection::open(&database_path).unwrap();
    migrations::run_migrations(&connection).unwrap();
    seed_conversation(&connection, "project-1", "conversation-1", "assistant-1");
    create_session(
        &mut connection,
        &create_input(session_id, "conversation-1", "assistant-1", "project-1", 10),
    )
    .unwrap();
    mark_running(&mut connection, "conversation-1", session_id, 11).unwrap();
    append_output(
        &mut connection,
        &AgentCommandSessionOutputAppend {
            conversation_id: "conversation-1",
            session_id,
            chunks: &[
                AgentCommandSessionOutputChunk {
                    sequence: 1,
                    stream: AgentCommandOutputStream::Stdout,
                    output: "before-crash-".to_string(),
                },
                AgentCommandSessionOutputChunk {
                    sequence: 2,
                    stream: AgentCommandOutputStream::Stderr,
                    output: "same-cut".to_string(),
                },
            ],
            latest_sequence: 2,
            transcript_truncated: false,
            output_capture_truncated: false,
            updated_at: 12,
        },
    )
    .unwrap();

    let first = read_or_create_model_read(
        &mut connection,
        &AgentCommandSessionModelReadRequest {
            conversation_id: "conversation-1",
            session_id,
            run_id: "run-retry",
            call_id: "call-retry",
            action: AgentCommandSessionAction::Poll,
            max_output_bytes: 1024,
            host_output_truncated: false,
            created_at: 13,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        first
            .chunks
            .iter()
            .map(|chunk| chunk.output.as_str())
            .collect::<String>(),
        "before-crash-same-cut"
    );
    assert_eq!(first.receipt.last_output_sequence, Some(2));
    drop(connection);

    let mut connection = Connection::open(&database_path).unwrap();
    migrations::run_migrations(&connection).unwrap();
    let retry_request = AgentCommandSessionModelReadRequest {
        conversation_id: "conversation-1",
        session_id,
        run_id: "run-retry",
        call_id: "call-retry",
        action: AgentCommandSessionAction::Poll,
        max_output_bytes: 1024,
        host_output_truncated: true,
        created_at: 99,
    };
    let replay = load_model_read(&connection, &retry_request)
        .unwrap()
        .unwrap();
    assert_eq!(replay, first);
    assert_eq!(
        read_or_create_model_read(&mut connection, &retry_request)
            .unwrap()
            .unwrap(),
        first
    );

    let mismatched_retry = AgentCommandSessionModelReadRequest {
        action: AgentCommandSessionAction::Interrupt,
        ..retry_request.clone()
    };
    assert!(load_model_read(&connection, &mismatched_retry).is_err());
    let next = read_or_create_model_read(
        &mut connection,
        &AgentCommandSessionModelReadRequest {
            run_id: "run-next",
            call_id: "call-next",
            created_at: 100,
            host_output_truncated: false,
            ..retry_request.clone()
        },
    )
    .unwrap()
    .unwrap();
    assert!(
        next.chunks.is_empty(),
        "a new ToolCall must see only new output"
    );

    for index in 0..=MAX_RETAINED_MODEL_READ_RECEIPTS_PER_SESSION {
        read_or_create_model_read(
            &mut connection,
            &AgentCommandSessionModelReadRequest {
                run_id: &format!("run-retained-{index}"),
                call_id: &format!("call-retained-{index}"),
                created_at: 101 + i64::try_from(index).unwrap(),
                host_output_truncated: false,
                ..retry_request.clone()
            },
        )
        .unwrap()
        .unwrap();
    }
    let receipt_count: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_command_session_model_read_receipts
             WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipt_count, MAX_RETAINED_MODEL_READ_RECEIPTS_PER_SESSION);

    chat_repository::delete_conversation(&connection, "conversation-1").unwrap();
    let receipt_count: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_command_session_model_read_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipt_count, 0);
}

#[test]
fn persisted_transcript_is_bounded_head_tail_and_reports_the_gap() {
    let mut connection = test_connection();
    seed_conversation(&connection, "project-1", "conversation-1", "assistant-1");
    let session_id = "cmd_00000000000000000000000000000003";
    create_session(
        &mut connection,
        &create_input(session_id, "conversation-1", "assistant-1", "project-1", 10),
    )
    .unwrap();
    let chunks = (1..=6)
        .map(|sequence| AgentCommandSessionOutputChunk {
            sequence,
            stream: AgentCommandOutputStream::Stdout,
            output: char::from(b'a' + sequence as u8 - 1)
                .to_string()
                .repeat(60 * 1024),
        })
        .collect::<Vec<_>>();
    append_output(
        &mut connection,
        &AgentCommandSessionOutputAppend {
            conversation_id: "conversation-1",
            session_id,
            chunks: &chunks,
            latest_sequence: 6,
            transcript_truncated: false,
            output_capture_truncated: false,
            updated_at: 12,
        },
    )
    .unwrap();

    let transcript = read_transcript(
        &connection,
        "conversation-1",
        session_id,
        0,
        MAX_PERSISTED_COMMAND_TRANSCRIPT_BYTES,
    )
    .unwrap()
    .unwrap();
    let bytes = transcript
        .chunks
        .iter()
        .map(|chunk| chunk.output.len())
        .sum::<usize>();
    assert!(bytes <= MAX_PERSISTED_COMMAND_TRANSCRIPT_BYTES);
    assert!(transcript.truncated_before);
    assert_eq!(transcript.chunks.first().unwrap().sequence, 1);
    assert_eq!(transcript.chunks.last().unwrap().sequence, 6);
    assert!(transcript
        .chunks
        .windows(2)
        .any(|pair| pair[1].sequence > pair[0].sequence + 1));
}

#[test]
fn archive_reference_is_restricted_to_the_same_conversation_and_message() {
    let mut connection = test_connection();
    seed_conversation(&connection, "project-1", "conversation-1", "assistant-1");
    seed_conversation(&connection, "project-2", "conversation-2", "assistant-2");
    let session_id = "cmd_00000000000000000000000000000004";
    create_session(
        &mut connection,
        &create_input(session_id, "conversation-1", "assistant-1", "project-1", 10),
    )
    .unwrap();
    let foreign_archive = conversation_history_archive_repository::store_archive(
        &mut connection,
        &ConversationHistoryArchiveInput {
            conversation_id: "conversation-2".to_string(),
            assistant_message_id: "assistant-2".to_string(),
            sequence: 1,
            call_id: "foreign-call".to_string(),
            tool: "run_command".to_string(),
            content_type: "text/plain".to_string(),
            content: "foreign output".to_string(),
            truncated_at_source: false,
            model_projection_truncated: false,
            archive_projection_truncated: false,
            created_at: 12,
        },
    )
    .unwrap();
    let terminal = AgentCommandSessionTerminalUpdate {
        conversation_id: "conversation-1",
        session_id,
        status: AgentCommandSessionStatus::Exited,
        ended_at: 20,
        exit_code: Some(0),
        latest_sequence: 0,
        transcript_truncated: false,
        output_capture_truncated: false,
        archive_ref: Some(&foreign_archive.archive_ref),
        terminal_reason: None,
        committed_at: 20,
    };
    assert!(commit_terminal(&mut connection, &terminal).is_err());
    assert_eq!(
        get_session(&connection, "conversation-1", session_id)
            .unwrap()
            .unwrap()
            .snapshot
            .status,
        AgentCommandSessionStatus::Starting
    );
}

#[test]
fn startup_reconciliation_only_returns_sessions_changed_by_that_call() {
    let mut connection = test_connection();
    seed_conversation(&connection, "project-1", "conversation-1", "assistant-1");
    seed_conversation(&connection, "project-2", "conversation-2", "assistant-2");
    let first = "cmd_00000000000000000000000000000005";
    create_session(
        &mut connection,
        &create_input(first, "conversation-1", "assistant-1", "project-1", 10),
    )
    .unwrap();
    let reconciled = reconcile_active_sessions_on_startup(&mut connection, 100).unwrap();
    assert_eq!(reconciled.len(), 1);
    assert_eq!(reconciled[0].snapshot.session_id, first);
    assert_eq!(
        reconciled[0].snapshot.status,
        AgentCommandSessionStatus::OutcomeUnknown
    );

    let second = "cmd_00000000000000000000000000000006";
    create_session(
        &mut connection,
        &create_input(second, "conversation-2", "assistant-2", "project-2", 50),
    )
    .unwrap();
    let reconciled_again = reconcile_active_sessions_on_startup(&mut connection, 100).unwrap();
    assert_eq!(reconciled_again.len(), 1);
    assert_eq!(reconciled_again[0].snapshot.session_id, second);
}

#[test]
fn deleting_a_conversation_cascades_sessions_and_transcript_chunks() {
    let mut connection = test_connection();
    seed_conversation(&connection, "project-1", "conversation-1", "assistant-1");
    let session_id = "cmd_00000000000000000000000000000007";
    create_session(
        &mut connection,
        &create_input(session_id, "conversation-1", "assistant-1", "project-1", 10),
    )
    .unwrap();
    append_output(
        &mut connection,
        &AgentCommandSessionOutputAppend {
            conversation_id: "conversation-1",
            session_id,
            chunks: &[AgentCommandSessionOutputChunk {
                sequence: 1,
                stream: AgentCommandOutputStream::Stdout,
                output: "still running".to_string(),
            }],
            latest_sequence: 1,
            transcript_truncated: false,
            output_capture_truncated: false,
            updated_at: 11,
        },
    )
    .unwrap();

    chat_repository::delete_conversation(&connection, "conversation-1").unwrap();
    let session_count: u64 = connection
        .query_row("SELECT COUNT(*) FROM agent_command_sessions", [], |row| {
            row.get(0)
        })
        .unwrap();
    let chunk_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_command_session_output_chunks",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((session_count, chunk_count), (0, 0));
}

#[test]
fn terminal_retention_prunes_operational_rows_but_preserves_exact_archive() {
    let mut connection = test_connection();
    seed_conversation(&connection, "project-1", "conversation-1", "assistant-1");
    let unresolved_id = "cmd_ffffffffffffffffffffffffffffffff";
    create_session(
        &mut connection,
        &create_input(
            unresolved_id,
            "conversation-1",
            "assistant-1",
            "project-1",
            10,
        ),
    )
    .unwrap();
    let unresolved =
        reconcile_active_session_in_connection(&connection, "conversation-1", unresolved_id, 20)
            .unwrap()
            .expect("active Session should become outcome unknown");
    assert_eq!(
        unresolved.snapshot.status,
        AgentCommandSessionStatus::OutcomeUnknown
    );
    let descriptor = conversation_history_archive_repository::store_archive(
        &mut connection,
        &ConversationHistoryArchiveInput {
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            sequence: 99,
            call_id: "archive-call".to_string(),
            tool: "run_command".to_string(),
            content_type: "text/plain".to_string(),
            content: "old exact output".to_string(),
            truncated_at_source: false,
            model_projection_truncated: true,
            archive_projection_truncated: false,
            created_at: 20,
        },
    )
    .unwrap();
    let oldest_id = "cmd_00000000000000000000000000000000";
    for index in 0..=MAX_RETAINED_TERMINAL_COMMAND_SESSIONS_PER_CONVERSATION {
        let session_id = format!("cmd_{index:032x}");
        let started_at = 30 + u64::try_from(index).unwrap();
        create_session(
            &mut connection,
            &create_input(
                &session_id,
                "conversation-1",
                "assistant-1",
                "project-1",
                started_at,
            ),
        )
        .unwrap();
        if index == 0 {
            append_output(
                &mut connection,
                &AgentCommandSessionOutputAppend {
                    conversation_id: "conversation-1",
                    session_id: &session_id,
                    chunks: &[AgentCommandSessionOutputChunk {
                        sequence: 1,
                        stream: AgentCommandOutputStream::Stdout,
                        output: "old bounded output".to_string(),
                    }],
                    latest_sequence: 1,
                    transcript_truncated: false,
                    output_capture_truncated: false,
                    updated_at: 50,
                },
            )
            .unwrap();
        }
        commit_terminal(
            &mut connection,
            &AgentCommandSessionTerminalUpdate {
                conversation_id: "conversation-1",
                session_id: &session_id,
                status: AgentCommandSessionStatus::Exited,
                ended_at: 1_000 + u64::try_from(index).unwrap(),
                exit_code: Some(0),
                latest_sequence: u64::from(index == 0),
                transcript_truncated: false,
                output_capture_truncated: false,
                archive_ref: (index == 0).then_some(descriptor.archive_ref.as_str()),
                terminal_reason: None,
                committed_at: 1_000 + i64::try_from(index).unwrap(),
            },
        )
        .unwrap();
    }

    let retained: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_command_sessions
             WHERE conversation_id = 'conversation-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        retained,
        (MAX_RETAINED_TERMINAL_COMMAND_SESSIONS_PER_CONVERSATION + 1) as u64,
        "the resolved terminal cap must not evict or consume the unresolved uncertainty row"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_command_sessions
                 WHERE conversation_id = 'conversation-1'
                   AND status != 'outcome_unknown'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        MAX_RETAINED_TERMINAL_COMMAND_SESSIONS_PER_CONVERSATION as u64
    );
    assert_eq!(
        get_session(&connection, "conversation-1", unresolved_id)
            .unwrap()
            .expect("unresolved Session must be pinned")
            .snapshot
            .status,
        AgentCommandSessionStatus::OutcomeUnknown
    );
    assert!(
        list_sessions_for_conversation(&connection, "conversation-1", 0)
            .unwrap()
            .is_empty()
    );
    let most_important_terminal =
        list_sessions_for_conversation(&connection, "conversation-1", 1).unwrap();
    assert_eq!(most_important_terminal.len(), 1);
    assert_eq!(
        most_important_terminal[0].snapshot.session_id,
        unresolved_id
    );
    assert!(get_session(&connection, "conversation-1", oldest_id)
        .unwrap()
        .is_none());
    let oldest_chunks: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_command_session_output_chunks
             WHERE session_id = ?1",
            [oldest_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(oldest_chunks, 0);
    assert!(
        conversation_history_archive_repository::find_archive_by_ref(
            &connection,
            "conversation-1",
            &descriptor.archive_ref,
        )
        .unwrap()
        .is_some()
    );
}
