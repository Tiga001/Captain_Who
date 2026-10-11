type RuntimeTerminalSettlementHook = Arc<dyn Fn() + Send + Sync>;
static BEFORE_RUNTIME_TERMINAL_SETTLEMENT: Mutex<Vec<(String, RuntimeTerminalSettlementHook)>> =
    Mutex::new(Vec::new());

fn run_before_runtime_terminal_settlement_hook(run_id: &str) {
    let hook = {
        let mut hooks = BEFORE_RUNTIME_TERMINAL_SETTLEMENT.lock().unwrap();
        hooks
            .iter()
            .position(|(id, _)| id == run_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

static BEFORE_RUNTIME_EXECUTION_FENCE: Mutex<Vec<(String, RuntimeTerminalSettlementHook)>> =
    Mutex::new(Vec::new());

fn run_before_runtime_execution_fence_hook(run_id: &str) {
    let hook = {
        let mut hooks = BEFORE_RUNTIME_EXECUTION_FENCE.lock().unwrap();
        hooks
            .iter()
            .position(|(id, _)| id == run_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

static BEFORE_RUNTIME_CHECKPOINT: Mutex<Vec<(String, RuntimeTerminalSettlementHook)>> =
    Mutex::new(Vec::new());

fn run_before_runtime_checkpoint_hook(run_id: &str) {
    let hook = {
        let mut hooks = BEFORE_RUNTIME_CHECKPOINT.lock().unwrap();
        hooks
            .iter()
            .position(|(id, _)| id == run_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

mod runtime_isolation_tests {
    use super::*;
    use mycopilot_core::storage::models::{ModelConfigRecord, ModelSettingsRecord};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn isolation_model_settings(url: &str) -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: url.into(),
            api_token: "synthetic-token".into(),
            search_mode: "disabled".into(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "isolation-model".into(),
                provider_model_id: "isolation-model".into(),
                display_name: "Isolation model".into(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                    mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".into(),
                cached_input_price: String::new(),
                output_price: "0".into(),
                enabled: true,
            }],
        }
    }

    struct ReleaseWorker(Option<std::sync::mpsc::Sender<()>>);
    impl Drop for ReleaseWorker {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    async fn assert_terminal_worker_isolation(close_after_reservation: bool, hold_recovery: bool) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &directory.path().join("runtime-isolation.sqlite"),
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (provider_started, started) = tokio::sync::oneshot::channel();
        let (reply, reply_allowed) = tokio::sync::oneshot::channel();
        let provider = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut bytes = [0_u8; 8192];
            loop {
                let count = socket.read(&mut bytes).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&bytes[..count]);
                if let Some(header_end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    if request.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            provider_started.send(()).unwrap();
            reply_allowed.await.unwrap();
            let body = concat!(
                "data: {\"id\":\"isolation\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"durable reply\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"isolation\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        storage
            .save_model_settings(isolation_model_settings(&format!(
                "http://{address}/v1/chat/completions"
            )))
            .unwrap();
        let mut service = AgentService::new_authorized_for_test(storage.clone());
        if hold_recovery {
            // One physical worker exposes a completion job that waits for receipt recovery.
            service.blocking_work.completion =
                crate::application::blocking_executor::BlockingExecutor::new(
                    "completion",
                    1,
                    108,
                    400 * 1024 * 1024,
                )
                .unwrap();
        }
        let (notifications, mut receiver) = crate::transport::outbound_channel();
        let started_turn = service
            .start_conversation_turn(
                AgentConversationTurnInput {
                    conversation_id: None,
                    project_id: None,
                    model_id: "isolation-model".into(),
                    context_window_indicator_enabled: false,
                    content: "Reply briefly.".into(),
                    attachments: Vec::new(),
                    folder_references: Vec::new(),
                    skills: Vec::new(),
                    title: None,
                    user_message_id: None,
                    assistant_message_id: None,
                    max_tokens: None,
                    temperature: None,
                    prompt_preferences: None,
                    permissions: AgentPermissions::default(),
                },
                notifications,
            )
            .unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered_tx = Mutex::new(Some(entered_tx));
        let (release, released) = std::sync::mpsc::channel();
        let released = Mutex::new(released);
        let release = ReleaseWorker(Some(release));
        BEFORE_RUNTIME_TERMINAL_SETTLEMENT.lock().unwrap().push((
            started_turn.run_id.clone(),
            Arc::new(move || {
                entered_tx
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(
                        std::thread::current()
                            .name()
                            .unwrap_or("unnamed")
                            .to_string(),
                    )
                    .unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            }),
        ));
        tokio::time::timeout(Duration::from_secs(10), started)
            .await
            .unwrap()
            .unwrap();
        if close_after_reservation {
            service.blocking_work.completion.close();
        }
        let (recovery_release, recovery_task) = if hold_recovery {
            // Exact-Run recovery no longer takes conversation admission. Occupy its existing
            // bounded worker before terminal settlement so the recovery barrier is real,
            // independent of whether the async delivery task happens to run before try_recv.
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            let release = ReleaseWorker(Some(release));
            let task = service
                .blocking_work
                .recovery
                .submit(0, move || {
                    entered_tx
                        .send(
                            std::thread::current()
                                .name()
                                .unwrap_or("unnamed")
                                .to_string(),
                        )
                        .unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                })
                .unwrap();
            let thread = tokio::time::timeout(Duration::from_secs(10), entered_rx)
                .await
                .unwrap()
                .unwrap();
            assert!(
                thread.starts_with("blocking-recovery-"),
                "receipt recovery blocker ran on {thread}"
            );
            (Some(release), Some(task))
        } else {
            (None, None)
        };
        reply.send(()).unwrap();
        let thread = tokio::time::timeout(Duration::from_secs(10), entered_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            thread.starts_with("blocking-completion-"),
            "settlement ran on {thread}"
        );
        // This test runs on a single Tokio thread. A blocked synchronous settlement must not
        // prevent its timer, transport task, or independent control code from progressing.
        tokio::time::timeout(
            Duration::from_millis(100),
            tokio::time::sleep(Duration::from_millis(20)),
        )
        .await
        .unwrap();
        assert_eq!(
            storage
                .get_conversation_turn_trace(&started_turn.assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status,
            ConversationTurnTraceTerminalStatus::InProgress
        );
        while let Ok(notification) = receiver.try_recv() {
            assert_ne!(
                notification["params"]["type"], "done",
                "terminal publication preceded commit: {notification}"
            );
        }
        drop(release);
        let mut held_completion_capacity = Vec::new();
        if hold_recovery {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if storage
                        .get_conversation_turn_trace(&started_turn.assistant_message_id)
                        .unwrap()
                        .unwrap()
                        .terminal_status
                        == ConversationTurnTraceTerminalStatus::Completed
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            // Exact-Run receipt recovery still awaits its worker, but a durable completed Turn
            // must release its exact owner already. Otherwise a later start incorrectly rejects
            // the completed conversation as active while unrelated recovery jobs are running.
            // The sole completion worker must also remain available for unrelated settlement.
            tokio::time::timeout(
                Duration::from_secs(1),
                service.blocking_work.completion.run(1, || ()),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                !service
                    .has_conversation_turn_occupancy(&started_turn.conversation_id)
                    .unwrap(),
                "receipt recovery must not extend completed Turn occupancy"
            );
            assert!(!service
                .cancellations
                .lock()
                .unwrap()
                .contains_key(&started_turn.run_id));
            while let Ok(notification) = receiver.try_recv() {
                assert_ne!(
                    notification["params"]["type"], "done",
                    "Done must still wait for receipt recovery and delivery: {notification}"
                );
            }
            // Retain every remaining completion slot until Done. Receipt recovery must finish
            // on its own worker after that worker releases; a per-Turn completion retry loop would
            // need another slot and deadlock here. The one retained slot is the handoff budget.
            for _ in 0..107 {
                held_completion_capacity
                    .push(service.blocking_work.completion.try_reserve(0).unwrap());
            }
        }
        drop(recovery_release);
        if let Some(task) = recovery_task {
            task.wait().await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let notification = receiver.recv().await.unwrap();
                assert_ne!(notification["params"]["type"], "error", "{notification}");
                if notification["params"]["type"] == "done" {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            storage
                .get_conversation_turn_trace(&started_turn.assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status,
            ConversationTurnTraceTerminalStatus::Completed
        );
        assert!(!service
            .has_conversation_turn_occupancy(&started_turn.conversation_id)
            .unwrap());
        provider.await.unwrap();
        drop(held_completion_capacity);
        service.blocking_work.preparation.shutdown().await;
        service.blocking_work.completion.shutdown().await;
        service.blocking_work.recovery.shutdown().await;
        service.blocking_work.maintenance.shutdown().await;
        service.blocking_work.control.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn terminal_persistence_wait_keeps_runtime_responsive_and_done_follows_commit() {
        assert_terminal_worker_isolation(false, false).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reserved_terminal_settlement_survives_completion_admission_close() {
        assert_terminal_worker_isolation(true, false).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn receipt_recovery_wait_does_not_occupy_completion_worker() {
        assert_terminal_worker_isolation(false, true).await;
    }

    async fn assert_stale_claim_is_rejected_before_runtime(
        replace_token: bool,
        stop_tree: bool,
        checkpoint_boundary: bool,
    ) {
        use mycopilot_core::storage::models::{ChatConversationRecord, ChatMessageRecord};
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("runtime-fence.sqlite");
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &database,
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        // No server task is necessary: a successful socket accept is itself a forbidden model
        // invocation. The test only ever targets this isolated loopback listener.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        storage
            .save_model_settings(isolation_model_settings(&format!(
                "http://{}/v1/chat/completions",
                listener.local_addr().unwrap()
            )))
            .unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "runtime-fence-root".into(),
                project_id: None,
                model_id: Some("isolation-model".into()),
                title: "Synthetic root".into(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        storage
            .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
                agent_id: "runtime-fence-root-agent".into(),
                conversation_id: "runtime-fence-root".into(),
                creation_request_id: "runtime-fence-root-create".into(),
                task_name: "Root".into(),
            })
            .unwrap();
        // Use a real root Turn transaction to seed inherited permissions; direct SQL below is
        // reserved solely for forcing the two lease races after child admission has committed.
        let (root, revision) = storage
            .load_conversation_for_turn("runtime-fence-root")
            .unwrap();
        let mut root = root.unwrap();
        root.updated_at = 2;
        root.messages.push(ChatMessageRecord {
            human_interaction_response: None,
            id: "runtime-fence-root-assistant".into(),
            role: "assistant".into(),
            content: String::new(),
            created_at: 2,
            status: Some("pending".into()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        });
        let trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
            "runtime-fence-root-run",
            "runtime-fence-root",
            "runtime-fence-root-assistant",
        );
        storage
            .save_conversation_and_begin_turn(
                root,
                revision,
                None,
                mycopilot_core::AgentTurnPermissionSource::HostAuthenticatedRoot(
                    AgentPermissions::default(),
                ),
                &trace,
                2,
                2,
            )
            .unwrap();
        let trace = mycopilot_core::completed_conversation_trace_without_items(
            "runtime-fence-root-run",
            "runtime-fence-root",
            "runtime-fence-root-assistant",
        );
        if !stop_tree {
            storage
                .finalize_chat_message_with_conversation_trace(
                    "runtime-fence-root",
                    "runtime-fence-root-assistant",
                    "seed",
                    Some("sent"),
                    "completed",
                    &trace,
                    2,
                    3,
                )
                .unwrap();
        }
        let child = storage
            .create_child_agent(&mycopilot_core::CreateChildAgentInput {
                parent_agent_id: "runtime-fence-root-agent".into(),
                creation_request_id: "runtime-fence-child-create".into(),
                task_name: "fenced_child".into(),
                task: "No execution is allowed after this claim changes.".into(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: mycopilot_core::AgentForkTurns::None,
            })
            .unwrap();
        let service =
            AgentService::try_new_with_startup_reconciliation(storage.clone(), false, None)
                .unwrap();
        service.grant_execution_access_for_test();
        // A controlled claim origin gives the existing 60s lease a short remaining lifetime;
        // the schema intentionally forbids shortening an already-active lease.
        let claimed_at =
            mycopilot_core::storage::now_ms() - 60_000 + if stop_tree { 60_000 } else { 2_000 };
        let claimed = storage
            .claim_next_dispatchable_agent_wake_at("original-runtime-claim", claimed_at)
            .unwrap()
            .unwrap();
        let lease_deadline = claimed.lease_expires_at.unwrap();
        let trusted = TrustedAgentWakeTurnStart::new(
            child.initial_wake.wake_id.clone(),
            child.agent.agent_id.clone(),
            child.agent.conversation_id.clone(),
            child.task_message.message_id.clone(),
            "original-runtime-claim".into(),
            child.collaboration_identity.clone(),
        )
        .unwrap()
        .with_global_permit(service.turn_concurrency_gate().try_acquire().unwrap());
        let (notifications, mut receiver) = crate::transport::outbound_channel();
        let turn = service
            .execute_turn(AgentTurnStart::AgentWake(trusted), notifications)
            .unwrap();
        // Current-thread scheduling guarantees the child task has not run before this hook is
        // installed. Block the physical host worker after all preparation, then replace/expire.
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered_tx = Mutex::new(Some(entered_tx));
        let (release, released) = std::sync::mpsc::channel();
        let released = Mutex::new(released);
        let release = ReleaseWorker(Some(release));
        let hooks = if checkpoint_boundary {
            &BEFORE_RUNTIME_CHECKPOINT
        } else {
            &BEFORE_RUNTIME_EXECUTION_FENCE
        };
        hooks.lock().unwrap().push((
            turn.run_id.clone(),
            Arc::new(move || {
                entered_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            }),
        ));
        tokio::time::timeout(Duration::from_secs(10), entered_rx)
            .await
            .unwrap()
            .unwrap();
        let mut replacement_token = None;
        let mut successor_checkpoint = None;
        if stop_tree {
            storage
                .begin_agent_tree_run_stop_by_root_agent(
                    "runtime-fence-root-agent",
                    "runtime-fence-root-run",
                )
                .unwrap()
                .unwrap();
            assert!(
                !service
                    .cancellations
                    .lock()
                    .unwrap()
                    .get(&turn.run_id)
                    .unwrap()
                    .is_cancelled(),
                "the fixture deliberately delays process-local cancellation delivery"
            );
        } else if replace_token {
            // Authoritative recovery atomically replaces the claim after expiry using its
            // controllable clock; direct token UPDATE would violate the schema's immutability.
            storage
                .recover_agent_wakes_at("replacement-runtime-claim", lease_deadline + 1)
                .unwrap();
            replacement_token = storage
                .get_agent_wake(&child.initial_wake.wake_id)
                .unwrap()
                .unwrap()
                .claim_token;
            assert_ne!(replacement_token.as_deref(), Some("original-runtime-claim"));
            if checkpoint_boundary {
                // Represent the successor's already-confirmed display owner. The old segment
                // must neither replace this generation nor reach the fake model.
                successor_checkpoint = Some(
                    storage
                        .begin_runtime_checkpoint(
                            &turn.conversation_id,
                            &turn.assistant_message_id,
                            &turn.run_id,
                        )
                        .unwrap()
                        .unwrap()
                        .generation,
                );
            }
        } else {
            while !checkpoint_boundary && mycopilot_core::storage::now_ms() <= lease_deadline {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        if checkpoint_boundary {
            // Occupy the real isolated writer after the original final lease check passed.
            // The checkpoint job must recheck after this queue wait, not reuse that check.
            let blocked_storage = storage.clone();
            let (writer_entered, writer_started) = tokio::sync::oneshot::channel();
            let (release_writer, writer_released) = std::sync::mpsc::channel();
            let writer = std::thread::spawn(move || {
                blocked_storage
                    .coordinate_runtime_control(move || {
                        writer_entered.send(()).unwrap();
                        writer_released
                            .recv_timeout(Duration::from_secs(10))
                            .unwrap();
                        Ok(())
                    })
                    .unwrap();
            });
            writer_started.await.unwrap();
            drop(release);
            if !replace_token {
                while mycopilot_core::storage::now_ms() <= lease_deadline {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            } else {
                // Yield to the segment so its synchronous checkpoint call queues behind the
                // held writer. The ownership assertions below do not rely on this duration.
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            release_writer.send(()).unwrap();
            writer.join().unwrap();
        } else {
            drop(release);
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if !service
                    .cancellations
                    .lock()
                    .unwrap()
                    .contains_key(&turn.run_id)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fenced runtime must retire without stale-owner settlement");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "an expired or replacement claim must never reach the model"
        );
        assert_eq!(
            storage
                .get_conversation_turn_trace(&turn.assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status,
            ConversationTurnTraceTerminalStatus::InProgress,
            "an old claim cannot terminalize the accepted Turn; durable recovery owns its outcome"
        );
        let mut saw_fence_error = false;
        while let Ok(notification) = receiver.try_recv() {
            let event = if notification["params"]["event"].is_object() {
                &notification["params"]["event"]
            } else {
                &notification["params"]
            };
            assert_ne!(
                event["type"], "done",
                "fenced owner cannot publish Done: {notification}"
            );
            saw_fence_error |= event["code"] == "agent_wake_execution_fenced";
        }
        assert!(
            saw_fence_error,
            "fencing must remain an explicit recoverable error"
        );
        let wake = storage
            .get_agent_wake(&child.initial_wake.wake_id)
            .unwrap()
            .unwrap();
        if replace_token {
            assert_eq!(
                wake.claim_token.as_deref(),
                replacement_token.as_deref(),
                "the failed old segment must not adopt or erase the replacement claim"
            );
        }
        if checkpoint_boundary {
            let stored = storage
                .load_conversation(&turn.conversation_id)
                .unwrap()
                .unwrap();
            let assistant = stored
                .messages
                .iter()
                .find(|message| message.id == turn.assistant_message_id)
                .unwrap();
            let projection: serde_json::Value =
                serde_json::from_str(assistant.agent_run_json.as_deref().unwrap()).unwrap();
            assert_eq!(
                projection["coreCheckpoint"]["generation"].as_str(),
                successor_checkpoint.as_deref(),
                "an invalid old claim cannot install or replace a checkpoint generation",
            );
        }
        service.blocking_work.preparation.shutdown().await;
        service.blocking_work.completion.shutdown().await;
        service.blocking_work.recovery.shutdown().await;
        service.blocking_work.maintenance.shutdown().await;
        service.blocking_work.control.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn expired_claim_after_host_preparation_does_not_start_runtime() {
        assert_stale_claim_is_rejected_before_runtime(false, false, false).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replaced_claim_after_host_preparation_does_not_start_runtime() {
        assert_stale_claim_is_rejected_before_runtime(true, false, false).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn durable_tree_stop_after_host_preparation_fences_before_token_delivery() {
        assert_stale_claim_is_rejected_before_runtime(false, true, false).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn checkpoint_writer_wait_expired_claim_preserves_generation_and_never_samples() {
        assert_stale_claim_is_rejected_before_runtime(false, false, true).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn checkpoint_writer_wait_replaced_claim_preserves_successor_and_never_samples() {
        assert_stale_claim_is_rejected_before_runtime(true, false, true).await;
    }

    async fn assert_completion_rejection_closes_steering(control_closed: bool) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &directory.path().join("completion-rejection.sqlite"),
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        storage
            .save_model_settings(isolation_model_settings(&format!(
                "http://{}/v1/chat/completions",
                listener.local_addr().unwrap()
            )))
            .unwrap();
        let service = AgentService::new_authorized_for_test(storage.clone());
        service.blocking_work.completion.close();
        if control_closed {
            service.blocking_work.control.close();
        }
        let (notifications, mut receiver) = crate::transport::outbound_channel();
        let turn = service
            .start_conversation_turn(
                AgentConversationTurnInput {
                    conversation_id: None,
                    project_id: None,
                    model_id: "isolation-model".into(),
                    context_window_indicator_enabled: false,
                    content: "Must not execute.".into(),
                    attachments: Vec::new(),
                    folder_references: Vec::new(),
                    skills: Vec::new(),
                    title: None,
                    user_message_id: None,
                    assistant_message_id: None,
                    max_tokens: None,
                    temperature: None,
                    prompt_preferences: None,
                    permissions: AgentPermissions::default(),
                },
                notifications,
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let notification = receiver.recv().await.unwrap();
                assert_ne!(notification["params"]["type"], "done", "{notification}");
                if notification["params"]["code"] == "conversation_trace_persistence_failed" {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(!service
            .cancellations
            .lock()
            .unwrap()
            .contains_key(&turn.run_id));
        {
            let active = service.active_runs.lock().unwrap();
            assert!(
                active.get(&turn.run_id).is_none_or(|control| matches!(
                    control.steer_state,
                    super::super::ActiveRunSteerState::Closed { .. }
                )),
                "a rejected completion reservation must stop accepting guidance"
            );
        }
        assert_eq!(
            storage
                .get_conversation_turn_trace(&turn.assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status,
            ConversationTurnTraceTerminalStatus::InProgress,
            "uncertain accepted work retains its exact durable recovery fence"
        );
        assert!(service
            .has_conversation_turn_occupancy(&turn.conversation_id)
            .unwrap());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        service.blocking_work.preparation.shutdown().await;
        service.blocking_work.completion.shutdown().await;
        service.blocking_work.recovery.shutdown().await;
        service.blocking_work.maintenance.shutdown().await;
        service.blocking_work.control.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completion_rejection_retires_steering_on_control_worker() {
        assert_completion_rejection_closes_steering(false).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn closed_control_keeps_recovery_fence_but_closes_steering() {
        assert_completion_rejection_closes_steering(true).await;
    }
}
