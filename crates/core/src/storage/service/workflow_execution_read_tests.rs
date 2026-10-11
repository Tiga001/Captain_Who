use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("workflow-read-pool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn wal_runtime_data_and_cursor_share_one_snapshot_during_a_concurrent_commit() {
    let fixture = Fixture::new();
    let service = Arc::new(
        StorageService::open_with_model_credentials(
            &fixture.0.join("state.sqlite"),
            Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    service.state.connection().unwrap().execute_batch(
        "INSERT INTO workflow_instances(instance_id,template_id,template_revision,name,color,revision,updated_at,last_request_json,definition_json)
         VALUES('instance','template',1,'Read pool test','#123456',1,1,'{}','{\"schemaVersion\":1,\"id\":\"instance\"}');
         INSERT INTO workflow_mail_events(instance_id,kind,created_at) VALUES('instance','members_changed',1);",
    ).unwrap();

    for mode in 0..3 {
        let old_sequence = service
            .workflow_execution_runtime_summary("instance", None)
            .unwrap()
            .sequence;
        let (started, start_received) = mpsc::channel();
        let (resume, resume_received) = mpsc::channel();
        let resume_received = Arc::new(Mutex::new(resume_received));
        let intercepted = Arc::new(AtomicBool::new(false));
        service
            .state
            .configure_read_connections_for_test(|connection| {
                let started = started.clone();
                let resumed = resume_received.clone();
                let intercepted = intercepted.clone();
                connection
                    .authorizer(Some(move |context: AuthContext<'_>| {
                        // The initial instance-existence SELECT must already have pinned the snapshot.
                        // Pause before the cursor SELECT while an independent writer commits an event.
                        if matches!(
                            context.action,
                            AuthAction::Read {
                                table_name: "workflow_mail_events",
                                ..
                            }
                        ) && !intercepted.swap(true, Ordering::AcqRel)
                        {
                            let _ = started.send(());
                            let _ = resumed.lock().unwrap().recv_timeout(Duration::from_secs(3));
                        }
                        Authorization::Allow
                    }))
                    .unwrap();
            });
        let reader_service = service.clone();
        let reader = std::thread::spawn(move || match mode {
            0 => reader_service.workflow_execution_runtime("instance"),
            1 => reader_service.workflow_execution_runtime_since("instance", Some(0)),
            _ => reader_service.workflow_execution_runtime_summary("instance", Some(0)),
        });
        let entered = start_received.recv_timeout(Duration::from_secs(3));
        let writer_result = service.state.connection().unwrap().execute(
            "INSERT INTO workflow_mail_events(instance_id,kind,created_at) VALUES('instance','members_changed',2)", [],
        );
        let _ = resume.send(());
        let snapshot = reader.join().unwrap().unwrap();
        service
            .state
            .configure_read_connections_for_test(|connection| {
                connection
                    .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
                    .unwrap();
            });
        entered.expect("reader must reach the controlled snapshot boundary");
        writer_result.expect("WAL writer must commit without waiting for the pinned read snapshot");
        assert_eq!(snapshot.sequence, old_sequence);
        assert_eq!(snapshot.events.last().unwrap().sequence, old_sequence);
        let fresh = service
            .workflow_execution_runtime_summary("instance", None)
            .unwrap();
        assert_eq!(fresh.sequence, old_sequence + 1);
        assert_eq!(fresh.events.last().unwrap().sequence, fresh.sequence);
    }
}
