use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
use std::sync::atomic::{AtomicBool, Ordering};

struct DiskFixture {
    state: StorageState,
    directory: tempfile::TempDir,
}

impl DiskFixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let state = StorageState::open(&directory.path().join("review.sqlite")).unwrap();
        state
            .write_sync(1024, StorageWriteClass::Normal, |connection| {
                connection.execute_batch(
                    "CREATE TABLE review_fixture(value INTEGER NOT NULL);
                     INSERT INTO review_fixture(value) VALUES (1);",
                )
            })
            .unwrap()
            .unwrap();
        Self { state, directory }
    }

    fn reopen(&self) -> Connection {
        Connection::open(self.directory.path().join("review.sqlite")).unwrap()
    }
}

fn deny_rollback(context: AuthContext<'_>) -> Authorization {
    if matches!(
        context.action,
        AuthAction::Transaction {
            operation: TransactionOperation::Rollback
        }
    ) {
        Authorization::Deny
    } else {
        Authorization::Allow
    }
}

#[tokio::test]
async fn unpolled_async_future_does_not_reserve_a_ticket_or_prevent_shutdown() {
    let fixture = DiskFixture::new();
    let executed = Arc::new(AtomicBool::new(false));
    let job_executed = Arc::clone(&executed);
    let unpolled = fixture
        .state
        .write_async(1024, StorageWriteClass::Normal, move |connection| {
            job_executed.store(true, Ordering::Release);
            connection.execute("UPDATE review_fixture SET value = 99", [])
        });
    // Keep the future alive across shutdown: dropping it first would conceal an eagerly
    // reserved, never-started job that otherwise prevents the writer from draining.
    tokio::time::timeout(Duration::from_secs(2), fixture.state.shutdown())
        .await
        .expect("unpolled futures are not accepted database jobs")
        .unwrap();
    assert!(unpolled.await.is_err());
    assert!(!executed.load(Ordering::Acquire));
    assert_eq!(
        fixture
            .reopen()
            .query_row("SELECT value FROM review_fixture", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn failed_writer_rollback_quarantines_the_connection_instead_of_reusing_its_transaction() {
    let fixture = DiskFixture::new();
    {
        let connection = fixture.state.connection().unwrap();
        connection
            .execute_batch("BEGIN; UPDATE review_fixture SET value = 99;")
            .unwrap();
        connection.authorizer(Some(deny_rollback)).unwrap();
    }
    assert!(fixture.state.connection().is_err());
    assert_eq!(
        fixture.state.shutdown().await.unwrap_err(),
        "storage_writer_quarantined"
    );
    assert!(fixture
        .state
        .write_sync(1024, StorageWriteClass::Control, |connection| {
            connection.execute("UPDATE review_fixture SET value = 100", [])
        })
        .is_err());
    // Closing the quarantined handle must discard its uncommitted write; no later owner may
    // accidentally commit the failed operation together with an unrelated mutation.
    assert_eq!(
        fixture
            .reopen()
            .query_row("SELECT value FROM review_fixture", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn failed_reader_rollback_closes_the_pool_without_hanging_shutdown() {
    let fixture = DiskFixture::new();
    {
        let reader = fixture.state.read_connection().unwrap();
        reader.execute_batch("BEGIN").unwrap();
        assert_eq!(
            reader
                .query_row("SELECT value FROM review_fixture", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        fixture
            .reopen()
            .execute("UPDATE review_fixture SET value = 2", [])
            .unwrap();
        reader.authorizer(Some(deny_rollback)).unwrap();
    }
    assert!(fixture.state.read_connection().is_err());
    tokio::time::timeout(Duration::from_secs(2), fixture.state.shutdown())
        .await
        .expect("discarding a failed snapshot releases its pool job")
        .unwrap();
    assert_eq!(
        fixture
            .reopen()
            .query_row("SELECT value FROM review_fixture", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        2
    );
}
