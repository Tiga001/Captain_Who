//! Real disk claims and the production preparation budget, with no model or tool dispatch.
use super::*;

struct RejectPreparation {
    work: BlockingWork,
    entered: AtomicBool,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl AgentWakeTurnExecutionPort for RejectPreparation {
    fn blocking_work(&self) -> Option<BlockingWork> {
        Some(self.work.clone())
    }

    fn start(
        &self,
        wake: &AgentWakeRequestRecord,
        _permit: AgentTurnConcurrencyPermit,
    ) -> Result<AgentWakeExecutionHandle, String> {
        if wake.agent_id == "child-a-0" {
            self.entered.store(true, Ordering::Release);
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .map_err(|error| error.to_string())?;
        }
        Err("isolated preparation fixture stops before model admission".into())
    }

    fn observe<'a>(
        &'a self,
        _handle: &'a AgentWakeExecutionHandle,
        _waiting_already_reported: bool,
    ) -> DispatchFuture<'a, Result<AgentWakeExecutionObservation, String>> {
        Box::pin(async { Err("fixture never admits a model Turn".into()) })
    }

    fn recovered_handle(
        &self,
        _wake: &AgentWakeRequestRecord,
    ) -> Result<AgentWakeExecutionHandle, String> {
        Err("fixture has no recovered model Turn".into())
    }

    fn retain_recovered_permit(&self, _run_id: &str) -> Option<AgentTurnConcurrencyPermit> {
        None
    }
    fn retire_recovered_turn_after_settlement(&self, _handle: &AgentWakeExecutionHandle) {}
    fn interrupt(&self, _run_id: &str) -> Result<AgentRunCancellationOutcome, String> {
        Ok(AgentRunCancellationOutcome::NoEffect)
    }
}

fn seed_root_queue(
    storage: &mycopilot_core::storage::service::StorageService,
    database: &std::path::Path,
    label: &str,
    count: usize,
) {
    let root = format!("root-{label}");
    let conversation = format!("conversation-{root}");
    storage
        .save_conversation(empty_conversation(&conversation))
        .unwrap();
    storage
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: root.clone(),
            conversation_id: conversation.clone(),
            creation_request_id: format!("ensure-{root}"),
            task_name: root.clone(),
        })
        .unwrap();
    let raw = rusqlite::Connection::open(database).unwrap();
    raw.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    for index in 0..count {
        let child = format!("child-{label}-{index}");
        let child_conversation = format!("conversation-{child}");
        storage
            .save_conversation(empty_conversation(&child_conversation))
            .unwrap();
        raw.execute(
            "INSERT INTO agent_nodes (
                agent_id, schema_version, root_agent_id, root_conversation_id,
                parent_agent_id, conversation_id, project_id, creation_request_id,
                task_name, task_path, model_config_id_snapshot, model_display_name_snapshot,
                model_supports_image_snapshot, model_context_window_tokens_snapshot,
                model_settings_revision_snapshot, provider_connection_revision_snapshot,
                provider_protocol_revision_snapshot, model_selection_source_snapshot,
                lifecycle, revision, created_at, updated_at
             ) VALUES (?1, 1, ?2, ?3, ?2, ?4, NULL, ?5, ?1, ?6,
                'model-a', 'Model A', 0, 32000, 'settings-v1', 'connection-v1',
                'protocol-v1', 'explicit', 'active', 1, 2, 2)",
            rusqlite::params![
                child,
                root,
                conversation,
                child_conversation,
                format!("spawn-{child}"),
                format!("/root/{child}")
            ],
        )
        .unwrap();
        storage
            .enqueue_agent_wake(&mycopilot_core::EnqueueAgentWakeInput {
                wake_id: format!("wake-{child}"),
                root_agent_id: root.clone(),
                agent_id: child.clone(),
                requester_agent_id: root.clone(),
                request_id: format!("request-{child}"),
                source_agent_message_id: None,
            })
            .unwrap();
    }
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("independent root/human preparation did not progress");
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_large_root_leaves_preparation_capacity_for_other_roots_and_human_work() {
    let fixture = tempfile::tempdir().unwrap();
    let database = fixture.path().join("fair-preparation.sqlite");
    let storage = Arc::new(
        mycopilot_core::storage::service::StorageService::open_with_model_credentials(
            &database,
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    // All eight older A Wakes precede B/C in the durable FIFO. A used to fill both workers
    // and all eight preparation reservations before B or an ordinary human could enter.
    seed_root_queue(&storage, &database, "a", 8);
    seed_root_queue(&storage, &database, "b", 1);
    seed_root_queue(&storage, &database, "c", 1);
    let work = BlockingWork::new().unwrap();
    let gate = AgentTurnConcurrencyGate::default_process_gate();
    let (release, released) = std::sync::mpsc::channel();
    let executor = Arc::new(RejectPreparation {
        work: work.clone(),
        entered: AtomicBool::new(false),
        release: Mutex::new(released),
    });
    let dispatcher = AgentDispatcher::start_with_clock(
        Arc::new(SqliteAgentDispatcherStore::new(Arc::clone(&storage))),
        executor.clone(),
        Arc::new(TestClock(AtomicI64::new(mycopilot_core::storage::now_ms()))),
        gate.clone(),
        AgentDispatcherConfig {
            idle_poll_interval: Duration::from_millis(5),
            wake_lease_renew_interval: Duration::from_millis(10),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();
    wait_for(|| executor.entered.load(Ordering::Acquire)).await;
    wait_for(|| {
        ["b", "c"].iter().all(|root| {
            storage
                .get_agent_wake(&format!("wake-child-{root}-0"))
                .unwrap()
                .unwrap()
                .status
                == AgentWakeStatus::Failed
        })
    })
    .await;
    assert_eq!(gate.limit(), DEFAULT_AGENT_GLOBAL_CONCURRENCY);
    // Exercise a human-owned job on the same physical preparation pool and logical Turn gate.
    // Its durable metadata publication is the deterministic stand-in for slow-provider setup.
    let human_permit = gate.try_acquire().unwrap();
    let human_storage = Arc::clone(&storage);
    let human = work
        .preparation
        .try_reserve(DISPATCH_PREPARATION_ESTIMATED_BYTES)
        .unwrap()
        .submit(move || {
            let _permit = human_permit;
            human_storage.save_conversation(empty_conversation("ordinary-human"))
        });
    tokio::time::timeout(Duration::from_secs(2), human.wait())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(storage
        .load_conversation_meta("ordinary-human")
        .unwrap()
        .is_some());
    for index in 1..8 {
        assert_eq!(
            storage
                .get_agent_wake(&format!("wake-child-a-{index}"))
                .unwrap()
                .unwrap()
                .status,
            AgentWakeStatus::Queued
        );
    }
    assert_eq!(
        storage
            .get_agent_wake("wake-child-a-0")
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Claimed
    );
    release.send(()).unwrap();
    wait_for(|| {
        (0..8).all(|index| {
            storage
                .get_agent_wake(&format!("wake-child-a-{index}"))
                .unwrap()
                .unwrap()
                .status
                == AgentWakeStatus::Failed
        })
    })
    .await;
    dispatcher.shutdown().await.unwrap();
    assert_eq!(gate.active(), 0);
}
