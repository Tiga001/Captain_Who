use super::*;
use crate::storage::{StorageState, StorageWriteClass};

struct Fixture {
    state: StorageState,
    directory: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let state = StorageState::open(&directory.path().join("archive.sqlite")).unwrap();
        state
            .write_sync(1024, StorageWriteClass::Normal, |c| {
                c.execute_batch(
            "INSERT INTO conversations(id,title,created_at,updated_at) VALUES('chat','archive',1,1);
             INSERT INTO messages(id,conversation_id,role,content,status,created_at,position)
             VALUES('assistant','chat','assistant','','sent',1,0);"
        )
            })
            .unwrap()
            .unwrap();
        Self { state, directory }
    }
    fn input(&self, content: &str) -> ConversationHistoryArchiveInput {
        ConversationHistoryArchiveInput {
            conversation_id: "chat".into(),
            assistant_message_id: "assistant".into(),
            sequence: 0,
            call_id: "call".into(),
            tool: "run_command".into(),
            content_type: "text/plain".into(),
            content: content.into(),
            truncated_at_source: false,
            model_projection_truncated: true,
            archive_projection_truncated: false,
            created_at: 2,
        }
    }
    fn file_input(&self) -> ConversationHistoryArchiveFileInput {
        ConversationHistoryArchiveFileInput {
            conversation_id: "chat".into(),
            assistant_message_id: "assistant".into(),
            sequence: 0,
            call_id: "call".into(),
            tool: "run_command".into(),
            content_type: "text/plain".into(),
            content_path: self.directory.path().join("capture.txt"),
            truncated_at_source: false,
            model_projection_truncated: true,
            archive_projection_truncated: false,
            created_at: 2,
        }
    }
    fn commit(
        &self,
        prepared: &PreparedArchive<'_>,
    ) -> rusqlite::Result<ConversationHistoryArchiveDescriptor> {
        self.state
            .write_sync(prepared.estimated_bytes(), StorageWriteClass::Normal, |c| {
                store_prepared_archive(c, prepared)
            })
            .unwrap()
    }
    fn observer(&self) -> Connection {
        Connection::open(self.directory.path().join("archive.sqlite")).unwrap()
    }
    fn assert_no_archive(&self) {
        let c = self.observer();
        for table in [
            "conversation_history_blobs",
            "conversation_history_blob_chunks",
            "conversation_history_index_entries",
        ] {
            assert_eq!(
                c.query_row(
                    &format!(
                        "SELECT count(*) FROM {table} WHERE {}",
                        if table == "conversation_history_index_entries" {
                            "ref_key LIKE 'archive:%'"
                        } else {
                            "1=1"
                        }
                    ),
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0,
                "{table}"
            );
        }
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM conversation_history_fts WHERE record_type='archive'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}

#[test]
fn prepared_file_is_owned_and_fts_and_chunks_use_the_same_frozen_bytes() {
    let fixture = Fixture::new();
    let input = fixture.file_input();
    let content = "exact-正文-".repeat(100_000);
    std::fs::write(&input.content_path, &content).unwrap();
    let prepared = prepare_archive_file(&input, 16 * 1024 * 1024).unwrap();
    assert!(prepared.estimated_bytes() >= content.len());
    std::fs::remove_file(&input.content_path).unwrap();
    let descriptor = fixture.commit(&prepared).unwrap();
    assert_eq!(descriptor.content_hash, content_hash(content.as_bytes()));
    let observer = fixture.observer();
    assert_eq!(
        read_complete_archive_content(&observer, &descriptor).unwrap(),
        content
    );
    assert_eq!(
        observer
            .query_row(
                "SELECT content FROM conversation_history_fts WHERE archive_ref=?1",
                [&descriptor.archive_ref],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        content
    );
}

#[test]
fn chunk_or_index_write_failure_rolls_back_blob_chunks_and_search_projection() {
    for (table, condition) in [
        ("conversation_history_blob_chunks", "NEW.chunk_index=1"),
        (
            "conversation_history_index_entries",
            "NEW.ref_key LIKE 'archive:%'",
        ),
    ] {
        let fixture = Fixture::new();
        let input = fixture.input(&"正文-lines\n".repeat(100_000));
        let prepared = prepare_archive(&input, 16 * 1024 * 1024).unwrap();
        fixture
            .state
            .write_sync(1024, StorageWriteClass::Normal, |c| {
                c.execute_batch(&format!(
            "CREATE TRIGGER injected_archive_failure BEFORE INSERT ON {table} WHEN {condition}
             BEGIN SELECT RAISE(ABORT,'injected archive failure'); END;"
        ))
            })
            .unwrap()
            .unwrap();
        assert!(fixture
            .commit(&prepared)
            .unwrap_err()
            .to_string()
            .contains("injected archive failure"));
        fixture.assert_no_archive();
    }
}

#[test]
fn deleted_owner_and_competing_identity_are_rechecked_after_preparation() {
    let fixture = Fixture::new();
    let input = fixture.input("first exact content");
    let first = prepare_archive(&input, 1024 * 1024).unwrap();
    let other = fixture.input("different exact content");
    let conflicting = prepare_archive(&other, 1024 * 1024).unwrap();
    let first_descriptor = fixture.commit(&first).unwrap();
    assert!(fixture
        .commit(&conflicting)
        .unwrap_err()
        .to_string()
        .contains("identity was reused"));
    assert_eq!(fixture.commit(&first).unwrap(), first_descriptor);
    fixture
        .state
        .write_sync(1024, StorageWriteClass::Control, |c| {
            c.execute("DELETE FROM conversations WHERE id='chat'", [])
        })
        .unwrap()
        .unwrap();
    assert!(fixture.commit(&first).is_err());
    fixture.assert_no_archive();
}

#[test]
fn invalid_utf8_and_preparation_budget_failure_leave_database_untouched() {
    let fixture = Fixture::new();
    let input = fixture.file_input();
    std::fs::write(&input.content_path, [0xff, 0xfe, 0]).unwrap();
    assert!(prepare_archive_file(&input, 1024 * 1024).is_err());
    std::fs::write(&input.content_path, "a".repeat(16_384)).unwrap();
    assert!(prepare_archive_file(&input, 8_192).is_err());
    let memory = fixture.input(&"b".repeat(16_384));
    assert!(prepare_archive(&memory, 8_192).is_err());
    fixture.assert_no_archive();
}

#[test]
fn same_size_capture_mutation_between_preparation_and_verification_is_rejected() {
    let fixture = Fixture::new();
    let input = fixture.file_input();
    std::fs::write(&input.content_path, "A".repeat(600_000)).unwrap();
    let result = prepare_archive_file_before_verification(&input, 2 * 1024 * 1024, || {
        std::fs::write(&input.content_path, "B".repeat(600_000)).unwrap();
    });
    assert!(
        matches!(result, Err(error) if error.to_string().contains("changed while it was being stored"))
    );
    fixture.assert_no_archive();
}
