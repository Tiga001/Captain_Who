type PreparingAdmissionHook = Arc<dyn Fn() + Send + Sync>;
static BEFORE_PREPARING_ADMISSION: Mutex<Vec<(String, PreparingAdmissionHook)>> =
    Mutex::new(Vec::new());
static AFTER_PREPARING_CLAIM: Mutex<Vec<(String, PreparingAdmissionHook)>> = Mutex::new(Vec::new());
static AFTER_TERMINAL_ACTIVITY: Mutex<Vec<(String, PreparingAdmissionHook)>> =
    Mutex::new(Vec::new());

fn run_after_terminal_activity_hook(conversation_id: &str) {
    let hook = {
        let mut hooks = AFTER_TERMINAL_ACTIVITY.lock().unwrap();
        hooks
            .iter()
            .position(|(id, _)| id == conversation_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

pub(super) fn run_before_preparing_admission_hook(conversation_id: &str) {
    let hook = {
        let mut hooks = BEFORE_PREPARING_ADMISSION.lock().unwrap();
        hooks
            .iter()
            .position(|(id, _)| id == conversation_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

pub(super) fn run_after_preparing_claim_hook(conversation_id: &str) {
    let hook = {
        let mut hooks = AFTER_PREPARING_CLAIM.lock().unwrap();
        hooks
            .iter()
            .position(|(id, _)| id == conversation_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

mod resource_preparation_tests {
    use super::*;
    use mycopilot_core::storage::models::{
        ChatConversationRecord, ModelConfigRecord, ModelSettingsRecord,
    };
    use mycopilot_protocol_rs::{ExecutionAccessReason, SetExecutionAccessInput};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const DEADLINE: Duration = Duration::from_secs(5);

    struct PreparationPause {
        entered: mpsc::Receiver<()>,
        release: Option<mpsc::Sender<()>>,
    }

    impl PreparationPause {
        fn install(conversation_id: &str) -> Self {
            Self::install_in(conversation_id, &BEFORE_PREPARING_ADMISSION)
        }

        fn install_in(
            conversation_id: &str,
            hooks: &Mutex<Vec<(String, PreparingAdmissionHook)>>,
        ) -> Self {
            let (entered_tx, entered) = mpsc::channel();
            let (release, released) = mpsc::channel();
            let released = Mutex::new(released);
            hooks.lock().unwrap().push((
                conversation_id.into(),
                Arc::new(move || {
                    let _ = entered_tx.send(());
                    // A failing main assertion must still let a scoped worker retire.
                    let _ = released
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(15));
                }),
            ));
            Self {
                entered,
                release: Some(release),
            }
        }

        fn wait(&self) {
            self.entered
                .recv_timeout(DEADLINE)
                .expect("preparation did not reach its snapshot boundary");
        }

        fn resume(&mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.send(());
            }
        }
    }

    impl Drop for PreparationPause {
        fn drop(&mut self) {
            self.resume();
        }
    }

    fn settings(url: &str) -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: url.into(),
            api_token: "synthetic-preparation-token".into(),
            search_mode: "disabled".into(),
            tavily_api_key: String::new(),
            models: ["preparing-model", "next-model"]
                .map(|id| ModelConfigRecord {
                    id: id.into(),
                    provider_model_id: id.into(),
                    display_name: id.into(),
                    api_url_override: None,
                    api_token_override: None,
                    supports_image: false,
                    context_window_tokens: Some(128_000),
                    provider_profile_config:
                        mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
                        ),
                    input_price: "0".into(),
                    cached_input_price: String::new(),
                    output_price: "0".into(),
                    enabled: true,
                })
                .to_vec(),
        }
    }

    fn fixture(url: &str) -> (tempfile::TempDir, Arc<StorageService>, AgentService) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &directory.path().join("preparing.sqlite"),
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        storage.save_model_settings(settings(url)).unwrap();
        let service = AgentService::new_authorized_for_test(storage.clone());
        (directory, storage, service)
    }

    fn input(conversation_id: &str, suffix: &str) -> AgentConversationTurnInput {
        AgentConversationTurnInput {
            conversation_id: Some(conversation_id.into()),
            project_id: None,
            model_id: "preparing-model".into(),
            context_window_indicator_enabled: false,
            content: "Synthetic preparation test.".into(),
            attachments: Vec::new(),
            folder_references: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some(format!("{conversation_id}-{suffix}-user")),
            assistant_message_id: Some(format!("{conversation_id}-{suffix}-assistant")),
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        }
    }

    fn seed_conversation(storage: &StorageService, id: &str) {
        storage
            .save_conversation(ChatConversationRecord {
                id: id.into(),
                project_id: None,
                model_id: Some("preparing-model".into()),
                title: "Original history".into(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
    }

    fn preparing_run(service: &AgentService, conversation_id: &str) -> String {
        service
            .preparing
            .snapshot()
            .into_iter()
            .find(|snapshot| snapshot.token.conversation_id == conversation_id)
            .expect("worker retains preparation ownership")
            .token
            .run_id
    }

    fn assert_unadmitted(storage: &StorageService, turn: &AgentConversationTurnInput) {
        assert!(storage
            .get_conversation_turn_trace(turn.assistant_message_id.as_deref().unwrap())
            .unwrap()
            .is_none());
        if let Some(conversation) = storage
            .load_conversation(turn.conversation_id.as_deref().unwrap())
            .unwrap()
        {
            assert!(conversation.messages.iter().all(|message| {
                Some(&message.id) != turn.user_message_id.as_ref()
                    && Some(&message.id) != turn.assistant_message_id.as_ref()
            }));
        }
    }

    #[test]
    fn preparing_independent_conversations_reach_snapshot_and_stop_without_global_wait() {
        let (_directory, storage, service) = fixture("http://127.0.0.1:9/v1/chat/completions");
        let first = input("preparing-independent-a", "first");
        let second = input("preparing-independent-b", "first");
        let mut a = PreparationPause::install(first.conversation_id.as_deref().unwrap());
        let mut b = PreparationPause::install(second.conversation_id.as_deref().unwrap());
        std::thread::scope(|scope| {
            let first_worker = scope.spawn(|| {
                service
                    .start_conversation_turn(first.clone(), crate::transport::outbound_channel().0)
            });
            a.wait();
            let second_worker = scope.spawn(|| {
                service
                    .start_conversation_turn(second.clone(), crate::transport::outbound_channel().0)
            });
            b.wait();
            assert_eq!(service.preparing.snapshot().len(), 2);
            let first_run = preparing_run(&service, first.conversation_id.as_deref().unwrap());
            let second_run = preparing_run(&service, second.conversation_id.as_deref().unwrap());
            assert!(service.cancel_run_checked(&first_run).unwrap());
            assert!(!service
                .preparing
                .snapshot_run(&second_run)
                .unwrap()
                .cancellation
                .is_cancelled());
            assert!(service.cancel_run_checked(&second_run).unwrap());
            // Stop acknowledges intent while both physical workers are still held.
            assert_eq!(service.preparing.snapshot().len(), 2);
            a.resume();
            b.resume();
            assert!(first_worker.join().unwrap().is_err());
            assert!(second_worker.join().unwrap().is_err());
        });
        assert!(service.preparing.snapshot().is_empty());
        assert_unadmitted(&storage, &first);
        assert_unadmitted(&storage, &second);
    }

    fn assert_access_change_rejects_preparation(relogin: bool) {
        let (_directory, storage, service) = fixture("http://127.0.0.1:9/v1/chat/completions");
        let turn = input(
            if relogin {
                "preparing-relogin"
            } else {
                "preparing-revocation"
            },
            "first",
        );
        let mut pause = PreparationPause::install(turn.conversation_id.as_deref().unwrap());
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                service
                    .start_conversation_turn(turn.clone(), crate::transport::outbound_channel().0)
            });
            pause.wait();
            let now = now_ms();
            service
                .set_execution_access(SetExecutionAccessInput {
                    revision: 2,
                    identity_epoch: 1,
                    reason: ExecutionAccessReason::AccountSignedOut,
                    issued_at: now,
                    valid_until: now,
                })
                .unwrap();
            if relogin {
                service
                    .set_execution_access(SetExecutionAccessInput {
                        revision: 3,
                        identity_epoch: 2,
                        reason: ExecutionAccessReason::Allowed,
                        issued_at: now,
                        valid_until: now + 60_000,
                    })
                    .unwrap();
                assert!(service.capture_execution_access().is_ok());
            }
            pause.resume();
            let error = worker.join().unwrap().unwrap_err();
            assert_eq!(
                error
                    .data()
                    .and_then(|data| data.get("code"))
                    .and_then(Value::as_str),
                Some(if relogin {
                    "ACCOUNT_LICENSE_UNAVAILABLE"
                } else {
                    "ACCOUNT_LOGIN_REQUIRED"
                }),
                "admission must preserve the structured Host lease denial: {error}"
            );
        });
        assert!(service.preparing.snapshot().is_empty());
        assert_unadmitted(&storage, &turn);
    }

    #[test]
    fn preparing_permission_revocation_is_ordered_before_initial_trace_commit() {
        assert_access_change_rejects_preparation(false);
    }

    #[test]
    fn preparing_relogin_does_not_revive_old_account_preparation() {
        assert_access_change_rejects_preparation(true);
    }

    fn assert_changed_snapshot_rejects_preparation(model_changed: bool) {
        let (_directory, storage, service) = fixture("http://127.0.0.1:9/v1/chat/completions");
        let conversation_id = if model_changed {
            "preparing-model-revision"
        } else {
            "preparing-history-revision"
        };
        seed_conversation(&storage, conversation_id);
        let turn = input(conversation_id, "first");
        let mut pause = PreparationPause::install(conversation_id);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                service
                    .start_conversation_turn(turn.clone(), crate::transport::outbound_channel().0)
            });
            pause.wait();
            let revision = storage.conversation_revision(conversation_id).unwrap();
            let mut changed = storage.load_conversation(conversation_id).unwrap().unwrap();
            changed.title = "Concurrent durable change".into();
            if model_changed {
                changed.model_id = Some("next-model".into());
            }
            storage.save_conversation(changed).unwrap();
            assert_ne!(
                storage.conversation_revision(conversation_id).unwrap(),
                revision
            );
            pause.resume();
            assert!(worker.join().unwrap().is_err());
        });
        let durable = storage.load_conversation(conversation_id).unwrap().unwrap();
        assert_eq!(durable.title, "Concurrent durable change");
        assert_eq!(
            durable.model_id.as_deref(),
            Some(if model_changed {
                "next-model"
            } else {
                "preparing-model"
            })
        );
        assert!(service.preparing.snapshot().is_empty());
        assert_unadmitted(&storage, &turn);
    }

    #[test]
    fn preparing_history_revision_change_rejects_stale_snapshot() {
        assert_changed_snapshot_rejects_preparation(false);
    }

    #[test]
    fn preparing_model_revision_change_rejects_stale_snapshot() {
        assert_changed_snapshot_rejects_preparation(true);
    }

    #[test]
    fn preparing_delete_fences_owner_before_drain_and_preserves_unrelated_work() {
        let (_directory, storage, service) = fixture("http://127.0.0.1:9/v1/chat/completions");
        let conversation_id = "preparing-delete";
        seed_conversation(&storage, conversation_id);
        let turn = input(conversation_id, "first");
        let independent = input("preparing-delete-independent", "first");
        let mut pause = PreparationPause::install(conversation_id);
        let mut other_pause =
            PreparationPause::install(independent.conversation_id.as_deref().unwrap());
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                service
                    .start_conversation_turn(turn.clone(), crate::transport::outbound_channel().0)
            });
            pause.wait();
            let run_id = preparing_run(&service, conversation_id);
            let (deleted_tx, deleted_rx) = mpsc::channel();
            let service_ref = &service;
            let deletion = scope.spawn(move || {
                deleted_tx
                    .send(service_ref.delete_conversation(conversation_id))
                    .unwrap();
            });
            let deadline = Instant::now() + DEADLINE;
            while !service
                .preparing
                .snapshot_run(&run_id)
                .unwrap()
                .cancellation
                .is_cancelled()
            {
                assert!(
                    Instant::now() < deadline,
                    "delete must publish its fence before waiting for preparation"
                );
                std::thread::yield_now();
            }
            assert!(
                deleted_rx.try_recv().is_err(),
                "deletion must wait for the physical owner"
            );
            assert!(storage
                .load_conversation(conversation_id)
                .unwrap()
                .is_some());
            let other_worker = scope.spawn(|| {
                service.start_conversation_turn(
                    independent.clone(),
                    crate::transport::outbound_channel().0,
                )
            });
            other_pause.wait();
            assert!(service
                .cancel_run_checked(&preparing_run(
                    &service,
                    independent.conversation_id.as_deref().unwrap()
                ))
                .unwrap());
            other_pause.resume();
            assert!(other_worker.join().unwrap().is_err());
            pause.resume();
            assert!(worker.join().unwrap().is_err());
            let deleted = deleted_rx.recv_timeout(DEADLINE).unwrap();
            assert!(deleted.is_ok(), "{deleted:?}");
            deletion.join().unwrap();
        });
        assert!(storage
            .load_conversation(conversation_id)
            .unwrap()
            .is_none());
        assert!(service.preparing.snapshot().is_empty());
        assert_unadmitted(&storage, &turn);
        assert_unadmitted(&storage, &independent);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preparing_same_conversation_concurrent_sends_admit_exactly_one_turn() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let provider = tokio::spawn(async move {
            let (_connection, _) = listener.accept().await.unwrap();
            // The accepted first Turn stays active while the competing send is rejected.
            std::future::pending::<()>().await;
        });
        let (_directory, storage, service) = fixture(&url);
        let first = input("preparing-same-conversation", "first");
        let second = input("preparing-same-conversation", "second");
        let mut pause = PreparationPause::install(first.conversation_id.as_deref().unwrap());
        let handle = tokio::runtime::Handle::current();
        let (first_notifications, _first_receiver) = crate::transport::outbound_channel();
        let (second_notifications, _second_receiver) = crate::transport::outbound_channel();
        let accepted = std::thread::scope(|scope| {
            let first_worker = scope.spawn(|| {
                let _runtime = handle.enter();
                service.start_conversation_turn(first.clone(), first_notifications)
            });
            pause.wait();
            let (started_tx, started_rx) = mpsc::channel();
            let (second_tx, second_rx) = mpsc::channel();
            let service_ref = &service;
            let second_ref = &second;
            let handle_ref = &handle;
            let second_worker = scope.spawn(move || {
                let _runtime = handle_ref.enter();
                started_tx.send(()).unwrap();
                second_tx
                    .send(
                        service_ref
                            .start_conversation_turn(second_ref.clone(), second_notifications),
                    )
                    .unwrap();
            });
            started_rx.recv_timeout(DEADLINE).unwrap();
            assert_eq!(service.preparing.snapshot().len(), 1);
            assert!(second_rx.try_recv().is_err());
            pause.resume();
            let accepted = first_worker.join().unwrap().unwrap();
            let rejected = second_rx.recv_timeout(DEADLINE).unwrap().unwrap_err();
            assert!(
                matches!(
                    rejected
                        .data()
                        .and_then(|data| data.get("code"))
                        .and_then(Value::as_str),
                    Some("conversation_busy" | "provider_transition_blocked")
                ),
                "{rejected}"
            );
            second_worker.join().unwrap();
            accepted
        });
        assert!(service.preparing.snapshot().is_empty());
        let durable = storage
            .load_conversation(&accepted.conversation_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            durable
                .messages
                .iter()
                .filter(|message| message.role == "user")
                .count(),
            1
        );
        assert_eq!(
            durable
                .messages
                .iter()
                .filter(|message| message.role == "assistant")
                .count(),
            1
        );
        assert_eq!(
            storage
                .get_conversation_turn_trace(&accepted.assistant_message_id)
                .unwrap()
                .unwrap()
                .run_id,
            accepted.run_id
        );
        assert_unadmitted(&storage, &second);
        assert!(service.cancel_run_checked(&accepted.run_id).unwrap());
        tokio::time::timeout(DEADLINE, async {
            loop {
                if storage
                    .get_conversation_turn_trace(&accepted.assistant_message_id)
                    .unwrap()
                    .unwrap()
                    .terminal_status
                    .is_terminal()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        service.shutdown_blocking_work().await.unwrap();
        provider.abort();
        let _ = provider.await;
    }

    #[test]
    fn preparing_model_switch_waits_for_same_conversation_and_runs_for_other_conversation() {
        let (_directory, storage, service) = fixture("http://127.0.0.1:9/v1/chat/completions");
        let conversation_id = "preparing-model-switch";
        let independent_id = "preparing-independent-model-switch";
        seed_conversation(&storage, conversation_id);
        seed_conversation(&storage, independent_id);
        let preflight = |id: &str| {
            service
                .preflight_provider_transition(AgentProviderTransitionPreflightInput {
                    conversation_id: id.into(),
                    target_model_id: "next-model".into(),
                    for_send: false,
                })
                .unwrap()
        };
        let planned = preflight(conversation_id);
        let independent_plan = preflight(independent_id);
        let transition =
            |plan: &AgentProviderTransitionPreflightOutput| AgentProviderTransitionStartInput {
                conversation_id: plan.conversation_id.clone(),
                target_model_id: plan.target_model_id.clone(),
                transition_token: plan.transition_token.clone().unwrap(),
            };
        let turn = input(conversation_id, "first");
        let mut pause = PreparationPause::install(conversation_id);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                service
                    .start_conversation_turn(turn.clone(), crate::transport::outbound_channel().0)
            });
            pause.wait();
            let (started_tx, started_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let service_ref = &service;
            let same_input = transition(&planned);
            let switch = scope.spawn(move || {
                started_tx.send(()).unwrap();
                done_tx
                    .send(service_ref.start_provider_transition(
                        same_input,
                        crate::transport::outbound_channel().0,
                    ))
                    .unwrap();
            });
            started_rx.recv_timeout(DEADLINE).unwrap();
            let independent = service
                .start_provider_transition(
                    transition(&independent_plan),
                    crate::transport::outbound_channel().0,
                )
                .unwrap();
            assert_eq!(independent.target_model_id, "next-model");
            assert_eq!(
                storage
                    .load_conversation(independent_id)
                    .unwrap()
                    .unwrap()
                    .model_id
                    .as_deref(),
                Some("next-model")
            );
            assert!(
                done_rx.try_recv().is_err(),
                "same-resource transition cannot pass a Preparing owner"
            );
            assert!(service
                .cancel_run_checked(&preparing_run(&service, conversation_id))
                .unwrap());
            pause.resume();
            assert!(worker.join().unwrap().is_err());
            let changed = done_rx.recv_timeout(DEADLINE).unwrap().unwrap();
            assert_eq!(changed.target_model_id, "next-model");
            switch.join().unwrap();
        });
        assert_eq!(
            storage
                .load_conversation(conversation_id)
                .unwrap()
                .unwrap()
                .model_id
                .as_deref(),
            Some("next-model")
        );
        assert_unadmitted(&storage, &turn);
    }

    fn workflow_conversations(storage: &StorageService) -> (String, String) {
        let node = |id: &str| {
            serde_json::json!({
                "kind":"agent", "id":id, "name":id, "x":0, "y":0,
                "permissionMode":"default", "modelConfigId":"preparing-model",
                "receives":"Synthetic input", "task":"Reply briefly", "delivers":"Synthetic result"
            })
        };
        let definition = serde_json::json!({
            "schemaVersion":1, "id":"preparation-template", "name":"Preparation isolation",
            "description":"", "background":"Synthetic preparation test",
            "nodes":[node("a"),node("b")], "viewport":{"x":0,"y":0,"zoom":1}
        });
        storage
            .workflow_request(
                serde_json::from_value(serde_json::json!({
                    "operation":"save", "definition":definition, "expectedRevision":0
                }))
                .unwrap(),
            )
            .unwrap();
        let response = storage.workflow_request(serde_json::from_value(serde_json::json!({
            "operation":"saveInstance", "id":"preparation-instance", "templateId":"preparation-template",
            "name":"Preparation isolation", "color":"#123456",
            "bindings":[{"nodeId":"a","conversationId":null},{"nodeId":"b","conversationId":null}],
            "expectedRevision":0, "expectedTemplateRevision":1
        })).unwrap()).unwrap();
        let bindings = &response.instances[0].bindings;
        let id = |node: &str| {
            bindings
                .iter()
                .find(|entry| entry.node_id == node)
                .unwrap()
                .conversation_id
                .clone()
        };
        (id("a"), id("b"))
    }

    async fn read_synthetic_request(socket: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;
        let mut request = Vec::new();
        let mut bytes = [0_u8; 8192];
        loop {
            let count = socket.read(&mut bytes).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&bytes[..count]);
            if let Some(end) = request.windows(4).position(|value| value == b"\r\n\r\n") {
                let length = String::from_utf8_lossy(&request[..end])
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    return;
                }
            }
        }
    }

    async fn reply_synthetic(socket: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncWriteExt;
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Synthetic complete\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_activity_releases_registry_before_writer_delete_and_drains_before_removal() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let provider = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_synthetic_request(&mut socket).await;
            reply_synthetic(&mut socket).await;
        });
        let (_directory, storage, service) = fixture(&url);
        let conversation_id = "terminal-writer-delete-order";
        let mut terminal = PreparationPause::install_in(conversation_id, &AFTER_TERMINAL_ACTIVITY);
        let (notifications, _receiver) = crate::transport::outbound_channel();
        let started = service
            .start_conversation_turn(input(conversation_id, "first"), notifications)
            .unwrap();
        terminal.wait();
        // This assertion also prevents a regression from entering an irrecoverable test
        // deadlock: a terminal worker must not retain this mutex when it queues storage work.
        assert!(
            service.deletion_lifecycle.try_lock().is_ok(),
            "terminal activity may retain a drain guard, never the registry mutex"
        );
        let cancellation = service
            .cancellations
            .lock()
            .unwrap()
            .get(&started.run_id)
            .unwrap()
            .clone();
        let (done_tx, done_rx) = mpsc::channel();
        crate::application::agent::completion::install_after_terminal_publication_hook(
            &started.run_id,
            Arc::new(move || {
                let _ = done_tx.send(());
            }),
        );
        let deleting_service = service.clone();
        let deletion = tokio::task::spawn_blocking(move || {
            deleting_service.delete_conversation(conversation_id)
        });
        tokio::time::timeout(DEADLINE, async {
            while !cancellation.is_cancelled() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("writer-owned deletion marker must pass the paused terminal activity");
        assert!(
            !deletion.is_finished(),
            "deletion must still drain the registered terminal activity"
        );
        let independent_storage = Arc::clone(&storage);
        tokio::time::timeout(
            DEADLINE,
            tokio::task::spawn_blocking(move || {
                seed_conversation(&independent_storage, "terminal-independent-write");
            }),
        )
        .await
        .expect("an unrelated writer job must pass while deletion drains the terminal")
        .unwrap();
        // Holding the actual terminal owner through the bounded drain must refuse removal;
        // cancellation alone cannot pretend that the paused persistence worker has retired.
        let refused = tokio::time::timeout(DEADLINE, deletion)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(
            refused.contains("active file-producing action"),
            "{refused}"
        );
        assert!(storage
            .load_conversation(conversation_id)
            .unwrap()
            .is_some());
        assert!(!storage
            .get_conversation_turn_trace(&started.assistant_message_id)
            .unwrap()
            .unwrap()
            .terminal_status
            .is_terminal());
        terminal.resume();
        done_rx
            .recv_timeout(DEADLINE)
            .expect("terminal persistence and publication must finish after activity release");
        assert!(storage
            .get_conversation_turn_trace(&started.assistant_message_id)
            .unwrap()
            .unwrap()
            .terminal_status
            .is_terminal());
        service.delete_conversation(conversation_id).unwrap();
        assert!(storage
            .load_conversation(conversation_id)
            .unwrap()
            .is_none());
        assert!(storage
            .load_conversation("terminal-independent-write")
            .unwrap()
            .is_some());
        service.shutdown_blocking_work().await.unwrap();
        tokio::time::timeout(DEADLINE, provider)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preparing_workflow_claim_survives_other_real_turn_completion_and_run_recovery() {
        use mycopilot_core::workflow_execution::{
            InputStatus, MailStatus, SendOutput, SendRequest,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let (first_started_tx, first_started) = tokio::sync::oneshot::channel();
        let (finish_first_tx, finish_first) = tokio::sync::oneshot::channel();
        let provider = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            read_synthetic_request(&mut first).await;
            first_started_tx.send(()).unwrap();
            finish_first.await.unwrap();
            reply_synthetic(&mut first).await;
            let (mut second, _) = listener.accept().await.unwrap();
            read_synthetic_request(&mut second).await;
            reply_synthetic(&mut second).await;
        });
        let (directory, storage, service) = fixture(&url);
        let (source_conversation, target_conversation) = workflow_conversations(&storage);
        let (notifications, _receiver) = crate::transport::outbound_channel();
        let source = service
            .start_conversation_turn(input(&source_conversation, "source"), notifications.clone())
            .unwrap();
        tokio::time::timeout(DEADLINE, first_started)
            .await
            .unwrap()
            .unwrap();
        let source_snapshot = storage
            .workflow_execution_snapshot_for_run(&source_conversation, &source.run_id)
            .unwrap()
            .unwrap();
        let receipt = storage
            .workflow_execution_send(&SendRequest {
                conversation_id: source_conversation.clone(),
                source_run_id: source.run_id.clone(),
                tool_call_id: "preparation-claim-mail".into(),
                execution_version: source_snapshot.execution_version,
                messages: vec![SendOutput {
                    target_node_id: "b".into(),
                    message: "Synthetic work for a preparing owner".into(),
                    reply_to_message_id: None,
                }],
                model_input: None,
                recipient_versions: Default::default(),
            })
            .unwrap();
        let workflow = storage
            .workflow_execution_load_input(&receipt.input_ids[0])
            .unwrap()
            .unwrap();
        let mut target_input = input(&target_conversation, "workflow");
        target_input.user_message_id = Some(format!("workflow-message-{}", workflow.id));
        let mut pause = PreparationPause::install_in(&target_conversation, &AFTER_PREPARING_CLAIM);
        let (source_done_tx, source_done_rx) = mpsc::channel();
        crate::application::agent::completion::install_after_terminal_publication_hook(
            &source.run_id,
            Arc::new(move || {
                let _ = source_done_tx.send(());
            }),
        );
        let runtime = tokio::runtime::Handle::current();
        let target = std::thread::scope(|scope| {
            let target_worker = scope.spawn(|| {
                let _runtime = runtime.enter();
                service.start_root_turn_with_workflow(
                    target_input.clone(),
                    None,
                    None,
                    None,
                    Some(workflow),
                    notifications.clone(),
                )
            });
            pause.wait();
            let preparing = storage
                .workflow_execution_load_input(&receipt.input_ids[0])
                .unwrap()
                .unwrap();
            assert_eq!(preparing.status, InputStatus::Claimed);
            assert_eq!(preparing.mail_status, MailStatus::Processing);
            assert_unadmitted(&storage, &target_input);
            assert_eq!(
                preparing.run_id.as_deref(),
                Some(preparing_run(&service, &target_conversation).as_str())
            );
            finish_first_tx.send(()).unwrap();
            // Done is emitted after the real runtime's deferred exact-Run recovery tail.
            source_done_rx
                .recv_timeout(DEADLINE)
                .expect("unrelated completed Turn must publish Done while B prepares");
            let after = storage
                .workflow_execution_load_input(&receipt.input_ids[0])
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::to_value(after).unwrap(),
                serde_json::to_value(preparing).unwrap()
            );
            pause.resume();
            target_worker.join().unwrap().unwrap()
        });
        tokio::time::timeout(DEADLINE, async {
            loop {
                let durable = storage
                    .workflow_execution_load_input(&receipt.input_ids[0])
                    .unwrap()
                    .unwrap();
                if durable.mail_status == MailStatus::Processed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            storage
                .get_conversation_turn_trace(&target.assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status,
            ConversationTurnTraceTerminalStatus::Completed
        );
        service.shutdown_blocking_work().await.unwrap();
        tokio::time::timeout(DEADLINE, provider)
            .await
            .unwrap()
            .unwrap();
        // Startup reconstruction preserves the committed exact receipt after disk reopen.
        let reopened = StorageService::open_with_model_credentials(
            &directory.path().join("preparing.sqlite"),
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap();
        reopened.workflow_execution_recover_claims().unwrap();
        assert_eq!(
            reopened
                .workflow_execution_load_input(&receipt.input_ids[0])
                .unwrap()
                .unwrap()
                .mail_status,
            MailStatus::Processed
        );
    }

    #[test]
    fn run_publication_slow_owner_only_orders_its_own_cancellation() {
        let (_directory, _storage, service) = fixture("http://127.0.0.1:9/v1/chat/completions");
        let first_token = AgentCancellationToken::new();
        let second_token = AgentCancellationToken::new();
        service.register_cancellation("publication-a", first_token.clone());
        service.register_cancellation("publication-b", second_token.clone());
        let publishing = service.run_publication.lock_conversation("publication-a");
        std::thread::scope(|scope| {
            let (started_tx, started_rx) = mpsc::channel();
            let (first_tx, first_rx) = mpsc::channel();
            let service_ref = &service;
            let first = scope.spawn(move || {
                started_tx.send(()).unwrap();
                first_tx
                    .send(
                        service_ref
                            .cancel_run_internal("publication-a")
                            .any_effect(),
                    )
                    .unwrap();
            });
            started_rx.recv_timeout(DEADLINE).unwrap();
            let (other_tx, other_rx) = mpsc::channel();
            let service_ref = &service;
            let second = scope.spawn(move || {
                // Both registry insertion and cancellation must progress on an unrelated Run.
                service_ref.register_cancellation("publication-c", AgentCancellationToken::new());
                other_tx
                    .send(
                        service_ref
                            .cancel_run_internal("publication-b")
                            .any_effect(),
                    )
                    .unwrap();
            });
            assert!(other_rx
                .recv_timeout(DEADLINE)
                .expect("independent cancellation waited on a publication owner"));
            assert!(second_token.is_cancelled());
            assert!(!first_token.is_cancelled());
            assert!(first_rx.try_recv().is_err());
            drop(publishing);
            assert!(first_rx.recv_timeout(DEADLINE).unwrap());
            assert!(first_token.is_cancelled());
            first.join().unwrap();
            second.join().unwrap();
        });
        service.unregister_cancellation_if_current("publication-a", &first_token);
        service.unregister_cancellation_if_current("publication-b", &second_token);
        service.unregister_cancellation("publication-c");
    }

    #[test]
    fn run_publication_shutdown_fence_cancels_late_generation_and_preserves_exact_retirement() {
        let (_directory, _storage, service) = fixture("http://127.0.0.1:9/v1/chat/completions");
        let old = AgentCancellationToken::new();
        let successor = AgentCancellationToken::new();
        service.register_cancellation("shutdown-generation", old.clone());
        let publishing = service
            .run_publication
            .lock_conversation("shutdown-generation");
        let (started_tx, started_rx) = mpsc::channel();
        let (registered_tx, registered_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let service_ref = &service;
            let successor_ref = &successor;
            let register = scope.spawn(move || {
                started_tx.send(()).unwrap();
                service_ref.register_cancellation("shutdown-generation", successor_ref.clone());
                registered_tx.send(()).unwrap();
            });
            started_rx.recv_timeout(DEADLINE).unwrap();
            service.runtime_stopping.store(true, Ordering::Release);
            assert!(registered_rx.try_recv().is_err());
            drop(publishing);
            registered_rx.recv_timeout(DEADLINE).unwrap();
            register.join().unwrap();
        });
        assert!(
            successor.is_cancelled(),
            "a resumed generation cannot escape a published shutdown fence"
        );
        service.unregister_cancellation_if_current("shutdown-generation", &old);
        assert!(service
            .cancellations
            .lock()
            .unwrap()
            .get("shutdown-generation")
            .unwrap()
            .shares_state_with(&successor));
        service.unregister_cancellation_if_current("shutdown-generation", &successor);
        assert!(!service
            .cancellations
            .lock()
            .unwrap()
            .contains_key("shutdown-generation"));
    }
}
