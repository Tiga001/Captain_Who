use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};

struct Fixture {
    service: AgentService,
    storage: Arc<StorageService>,
    directory: tempfile::TempDir,
}
impl Fixture {
    fn new(generator: ContextCompactionSummaryGenerator) -> Self {
        let directory = tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &directory.path().join("shutdown.sqlite"),
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        storage
            .save_model_settings(two_model_settings(Some(
                mycopilot_core::ProviderProfileConfig::deepseek_flash_default(),
            )))
            .unwrap();
        let conversation =
            conversation_with_completed_history("transition-shutdown", Some("model-1"));
        let assistant = conversation.messages[1].id.clone();
        storage.save_conversation(conversation).unwrap();
        persist_completed_history(&storage, "transition-shutdown", &assistant);
        let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
            .with_context_compaction_summary_generator(generator);
        Self {
            service,
            storage,
            directory,
        }
    }
    fn start(&self) -> AgentProviderTransitionOperation {
        self.try_start().unwrap()
    }
    fn try_start(&self) -> Result<AgentProviderTransitionOperation, AgentServiceError> {
        let preflight = self
            .service
            .preflight_provider_transition(AgentProviderTransitionPreflightInput {
                for_send: false,
                conversation_id: "transition-shutdown".into(),
                target_model_id: "model-2".into(),
            })
            .unwrap();
        assert_eq!(
            preflight.decision,
            AgentProviderTransitionDecision::RequiresCompaction
        );
        let (notifications, _) = crate::transport::outbound_channel();
        self.service.start_provider_transition(
            AgentProviderTransitionStartInput {
                conversation_id: "transition-shutdown".into(),
                target_model_id: "model-2".into(),
                transition_token: preflight.transition_token.unwrap(),
            },
            notifications,
        )
    }
    fn observer(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.directory.path().join("shutdown.sqlite")).unwrap()
    }
    fn assert_receipt(&self, operation: &str, status: &str) {
        let connection = self.observer();
        assert_eq!(
            connection
                .query_row(
                    "SELECT status FROM context_compaction_receipts WHERE operation_id=?1",
                    [operation],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            status
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id FROM conversations WHERE id='transition-shutdown'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "model-1"
        );
    }
}

#[tokio::test]
async fn shutdown_before_first_poll_never_starts_the_model_and_preserves_cancelled_receipt() {
    let calls = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::clone(&calls);
    let fixture = Fixture::new(Arc::new(move |_, _| {
        recorded.fetch_add(1, Ordering::AcqRel);
        Box::pin(std::future::pending())
    }));
    let operation = fixture.start();
    fixture.service.provider_transition_tasks.begin_shutdown();
    fixture.service.shutdown_blocking_work().await.unwrap();
    fixture.storage.shutdown_storage().await.unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 0);
    fixture.assert_receipt(&operation.operation_id, "cancelled");
}

fn pending_generator(
    started: Arc<Notify>,
    dropped: Arc<AtomicBool>,
) -> ContextCompactionSummaryGenerator {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    Arc::new(move |_, _| {
        let started = Arc::clone(&started);
        let dropped = Dropped(Arc::clone(&dropped));
        Box::pin(async move {
            let _dropped = dropped;
            started.notify_one();
            std::future::pending().await
        })
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_owns_a_noncooperative_model_wait_and_rejects_new_transitions() {
    let started = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let fixture = Fixture::new(pending_generator(
        Arc::clone(&started),
        Arc::clone(&dropped),
    ));
    let operation = fixture.start();
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    let (cancelled, timed_out) = fixture
        .service
        .shutdown_active_runs(Duration::from_secs(2))
        .await;
    assert_eq!(
        cancelled, 0,
        "provider transitions are not counted as logical Agent runs"
    );
    assert!(!timed_out);
    fixture.service.shutdown_blocking_work().await.unwrap();
    assert!(dropped.load(Ordering::Acquire));
    assert_eq!(fixture.service.provider_transition_tasks.active(), 0);
    fixture.assert_receipt(&operation.operation_id, "cancelled");
    let (notifications, _) = crate::transport::outbound_channel();
    let error = fixture
        .service
        .start_provider_transition(
            AgentProviderTransitionStartInput {
                conversation_id: "transition-shutdown".into(),
                target_model_id: "model-2".into(),
                transition_token: "still-stopping".into(),
            },
            notifications,
        )
        .unwrap_err();
    assert!(error.to_string().contains("shutting down"));
    fixture.storage.shutdown_storage().await.unwrap();
}

#[tokio::test]
async fn shutdown_racing_a_ready_generation_keeps_known_observation_and_usage() {
    let base = provider_transition_generator("deepseek-flash");
    let fixture = Fixture::new(Arc::new(move |request, cancellation| {
        let base = Arc::clone(&base);
        Box::pin(async move {
            let mut result = base(request, cancellation.clone()).await?;
            result.observation.actual_usage = Some(mycopilot_core::ModelRequestActualUsage {
                raw: mycopilot_core::AgentUsage {
                    input_tokens: Some(100),
                    output_tokens: Some(10),
                    output_thinking_tokens: None,
                    total_tokens: Some(110),
                    cached_input_tokens: None,
                    cache_creation_input_tokens: None,
                    billable_request_count: Some(1),
                },
                normalized_input_tokens: Some(100),
                normalization: mycopilot_core::ModelRequestUsageNormalization::OpenAiInputTokens,
            });
            cancellation.cancel();
            Ok(result)
        })
    }));
    let operation = fixture.start();
    // Let the controllable generation return with an observation before joining the owner.
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.service.provider_transition_tasks.active() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.service.shutdown_blocking_work().await.unwrap();
    fixture.assert_receipt(&operation.operation_id, "cancelled");
    let connection = fixture.observer();
    assert_eq!(connection.query_row(
        "SELECT normalized_actual_input_tokens FROM model_request_observations WHERE operation_id=?1",
        [&operation.operation_id], |row| row.get::<_,i64>(0)).unwrap(), 100);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM model_request_observations WHERE operation_id=?1",
                [&operation.operation_id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    fixture.storage.shutdown_storage().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_waits_for_started_receipt_write_and_reports_failure_without_losing_recovery() {
    for fail in [false, true] {
        let started = Arc::new(Notify::new());
        let fixture = Fixture::new(pending_generator(
            Arc::clone(&started),
            Arc::new(AtomicBool::new(false)),
        ));
        let operation = fixture.start();
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        let blocker = fixture.observer();
        if fail {
            blocker.execute_batch("CREATE TRIGGER reject_transition_terminal BEFORE UPDATE ON context_compaction_receipts WHEN NEW.status != 'in_progress' BEGIN SELECT RAISE(ABORT,'injected terminal failure'); END;").unwrap();
        }
        // Hold the real service writer in an admitted transaction. An external deferred
        // reader upgrading under BEGIN IMMEDIATE can fail BUSY immediately in WAL mode;
        // it does not exercise ownership of a synchronous operation waiting for the gate.
        let writer_gate = Arc::new((Mutex::new(false), Condvar::new()));
        struct ReleaseWriter(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for ReleaseWriter {
            fn drop(&mut self) {
                *self.0 .0.lock().unwrap() = true;
                self.0 .1.notify_all();
            }
        }
        let release_writer = ReleaseWriter(Arc::clone(&writer_gate));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered_tx = Mutex::new(Some(entered_tx));
        let storage = Arc::clone(&fixture.storage);
        let writer = std::thread::spawn(move || {
            let conversation = conversation_with_completed_history("writer-gate", Some("model-1"));
            let trace = ConversationTraceSnapshot::default().in_progress_trace(
                "writer-gate-run",
                "writer-gate",
                &conversation.messages[1].id,
            );
            let error = storage
                .save_conversation_and_begin_turn_with_execution_access(
                    conversation,
                    None,
                    None,
                    mycopilot_core::AgentTurnPermissionSource::HostAuthenticatedRoot(
                        AgentPermissions::default(),
                    ),
                    &[],
                    &trace,
                    1,
                    1,
                    Some(&|| {
                        entered_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                        let (released, timeout) = writer_gate
                            .1
                            .wait_timeout_while(
                                writer_gate.0.lock().unwrap(),
                                Duration::from_secs(3),
                                |released| !*released,
                            )
                            .unwrap();
                        assert!(
                            *released && !timeout.timed_out(),
                            "writer gate was not released"
                        );
                        Err("fixture writer released without mutations".to_string())
                    }),
                )
                .unwrap_err();
            assert_eq!(error, "fixture writer released without mutations");
        });
        tokio::time::timeout(Duration::from_secs(2), entered_rx)
            .await
            .unwrap()
            .unwrap();
        let service = fixture.service.clone();
        let closing_finished = Arc::new(AtomicBool::new(false));
        let completed = Arc::clone(&closing_finished);
        let heartbeat = Arc::new(AtomicBool::new(false));
        let observed_heartbeat = Arc::clone(&heartbeat);
        let (released_tx, released_rx) = tokio::sync::oneshot::channel();
        // Release from an independent OS thread even if the regression blocks Tokio's timer
        // driver. The sample, taken before release, separately asserts scheduler progress and
        // ownership of the started synchronous write; no longer timeout poisons the writer.
        let release_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(180));
            let finished_too_soon = closing_finished.load(Ordering::Acquire);
            let heartbeat_progressed = observed_heartbeat.load(Ordering::Acquire);
            drop(release_writer);
            released_tx
                .send((finished_too_soon, heartbeat_progressed))
                .unwrap();
        });
        let closing = tokio::spawn(async move {
            let result = service.shutdown_blocking_work().await;
            completed.store(true, Ordering::Release);
            result
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        heartbeat.store(true, Ordering::Release);
        let (finished_too_soon, heartbeat_progressed) = released_rx.await.unwrap();
        release_thread.join().unwrap();
        writer.join().unwrap();
        assert!(
            !finished_too_soon,
            "closing must retain ownership of the synchronous receipt write"
        );
        assert!(
            heartbeat_progressed,
            "waiting for receipt settlement must not block Tokio scheduling"
        );
        let result = tokio::time::timeout(Duration::from_secs(2), closing)
            .await
            .unwrap()
            .unwrap();
        if fail {
            assert!(result
                .unwrap_err()
                .contains("terminal receipt was not persisted"));
            fixture.assert_receipt(&operation.operation_id, "in_progress");
        } else {
            result.unwrap();
            fixture.assert_receipt(&operation.operation_id, "cancelled");
        }
        assert_eq!(fixture.service.provider_transition_tasks.active(), 0);
        fixture.storage.shutdown_storage().await.unwrap();
    }
}

#[tokio::test]
async fn physical_capacity_is_reserved_before_claim_or_receipt_acceptance() {
    use crate::application::blocking_executor::BlockingError;
    for completion in [false, true] {
        let fixture = Fixture::new(Arc::new(|_, _| Box::pin(std::future::pending())));
        let executor = if completion {
            &fixture.service.blocking_work.completion
        } else {
            &fixture.service.blocking_work.preparation
        };
        let mut held = Vec::new();
        loop {
            match executor.try_reserve(0) {
                Ok(reservation) => held.push(reservation),
                Err(BlockingError::Overloaded) => break,
                Err(error) => panic!("unexpected admission error: {error}"),
            }
        }
        assert!(fixture
            .try_start()
            .unwrap_err()
            .to_string()
            .contains("无法启动历史压缩"));
        assert_eq!(fixture.service.provider_transition_tasks.active(), 0);
        assert!(fixture
            .service
            .provider_transitions
            .lock()
            .unwrap()
            .is_empty());
        let receipts: i64 = fixture
            .observer()
            .query_row(
                "SELECT COUNT(*) FROM context_compaction_receipts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receipts, 0, "overload cannot publish durable acceptance");
        drop(held);
        let operation = fixture.start();
        fixture.service.shutdown_blocking_work().await.unwrap();
        fixture.assert_receipt(&operation.operation_id, "cancelled");
        fixture.storage.shutdown_storage().await.unwrap();
    }
}

#[tokio::test]
async fn completion_resize_refusal_never_commits_and_reports_unpersistable_known_usage() {
    use crate::application::blocking_executor::BlockingExecutor;
    for oversized_observation in [false, true] {
        let base = provider_transition_generator("deepseek-flash");
        let mut fixture = Fixture::new(Arc::new(move |request, cancellation| {
            let base = Arc::clone(&base);
            Box::pin(async move {
                let mut result = base(request, cancellation).await?;
                result.draft.content = "x".repeat(256 * 1024);
                result.observation.actual_usage = Some(mycopilot_core::ModelRequestActualUsage {
                    raw: mycopilot_core::AgentUsage {
                        input_tokens: Some(100),
                        output_tokens: Some(10),
                        total_tokens: Some(110),
                        billable_request_count: Some(1),
                        output_thinking_tokens: None,
                        cached_input_tokens: None,
                        cache_creation_input_tokens: None,
                    },
                    normalized_input_tokens: Some(100),
                    normalization:
                        mycopilot_core::ModelRequestUsageNormalization::OpenAiInputTokens,
                });
                if oversized_observation {
                    result.observation.finish_reason = Some("x".repeat(256 * 1024));
                }
                Ok(result)
            })
        }));
        let previous = std::mem::replace(
            &mut fixture.service.blocking_work.completion,
            BlockingExecutor::new("transition-resize-fixture", 1, 4, 128 * 1024).unwrap(),
        );
        previous.shutdown_submitted().await;
        let operation = fixture.start();
        tokio::time::timeout(Duration::from_secs(2), async {
            while fixture.service.provider_transition_tasks.active() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let result = fixture.service.shutdown_blocking_work().await;
        if oversized_observation {
            assert!(result
                .unwrap_err()
                .contains("durable receipt retained for recovery"));
            fixture.assert_receipt(&operation.operation_id, "in_progress");
        } else {
            result.unwrap();
            fixture.assert_receipt(&operation.operation_id, "failed");
            let observed: i64 = fixture.observer().query_row(
                "SELECT normalized_actual_input_tokens FROM model_request_observations WHERE operation_id=?1",
                [&operation.operation_id], |row| row.get(0),
            ).unwrap();
            assert_eq!(observed, 100, "capacity failure must preserve known usage");
        }
        let summaries: i64 = fixture
            .observer()
            .query_row(
                "SELECT COUNT(*) FROM context_compaction_summaries",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            summaries, 0,
            "oversized completion must never commit a model switch"
        );
        fixture.storage.shutdown_storage().await.unwrap();
    }
}
