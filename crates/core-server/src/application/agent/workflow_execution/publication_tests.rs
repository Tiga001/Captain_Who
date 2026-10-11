use super::*;
use mycopilot_core::storage::models::{ModelConfigRecord, ModelSettingsRecord};
use mycopilot_core::workflow_execution::SendOutput;
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};

type SendHook = Arc<dyn Fn() + Send + Sync>;
static SEND_HOOKS: OnceLock<Mutex<HashMap<String, SendHook>>> = OnceLock::new();

pub(super) fn before_send_write(run_id: &str) {
    let hook = SEND_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(run_id);
    if let Some(hook) = hook {
        hook();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_send_waiting_for_writer_only_orders_its_own_stop() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open_with_model_credentials(
            &directory.path().join("workflow-publication.sqlite"),
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    storage
        .save_model_settings(ModelSettingsRecord {
            api_url: "https://example.test/v1/chat/completions".into(),
            api_token: "fixture".into(),
            search_mode: "disabled".into(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "fixture-model".into(),
                provider_model_id: "fixture-model".into(),
                display_name: "Fixture".into(),
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
        })
        .unwrap();
    let node = |id| {
        json!({"kind":"agent","id":id,"name":id,"x":0,"y":0,
        "permissionMode":"default","modelConfigId":"fixture-model",
        "receives":"Mail","task":"Review","delivers":"Reply"})
    };
    storage
        .workflow_request(
            serde_json::from_value(json!({"operation":"save",
        "definition":{"schemaVersion":1,"id":"template","name":"Fixture","description":"",
            "background":"Fixture","nodes":[node("a"),node("b")],"viewport":{"x":0,"y":0,"zoom":1}},
        "expectedRevision":0}))
            .unwrap(),
        )
        .unwrap();
    let created = storage
        .workflow_request(
            serde_json::from_value(json!({"operation":"saveInstance",
        "id":"instance","templateId":"template","name":"Fixture","color":"#123456",
        "bindings":[{"nodeId":"a","conversationId":null},{"nodeId":"b","conversationId":null}],
        "expectedRevision":0,"expectedTemplateRevision":1}))
            .unwrap(),
        )
        .unwrap();
    let conversation = |node: &str| {
        created.instances[0]
            .bindings
            .iter()
            .find(|binding| binding.node_id == node)
            .unwrap()
            .conversation_id
            .clone()
    };
    let source = conversation("a");
    let target = conversation("b");
    // Startup recovery finishes before this fixture admits its live execution segment.
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = format!("workflow-publication-{}", uuid::Uuid::new_v4());
    storage
        .upsert_chat_messages(
            &source,
            vec![ChatMessageRecord {
                id: "source-assistant".into(),
                role: "assistant".into(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".into()),
                attachments: vec![],
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
                human_interaction_response: None,
            }],
            0,
        )
        .unwrap();
    storage
        .append_in_progress_conversation_turn_trace(
            &mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
                &run_id,
                &source,
                "source-assistant",
            ),
            1,
            1,
        )
        .unwrap();
    let binding = storage
        .workflow_execution_bind_run(&source, &run_id)
        .unwrap()
        .unwrap();
    service
        .ensure_conversation_turn_owner(&source, &run_id, "source-assistant")
        .unwrap();
    service
        .reserve_conversation_turn(&target, "other-run", "other-assistant")
        .unwrap();
    let token = AgentCancellationToken::default();
    let other_token = AgentCancellationToken::default();
    service.register_cancellation(&run_id, token.clone());
    service.register_cancellation("other-run", other_token.clone());
    let host = StoredWorkflowRuntime {
        service: service.clone(),
        conversation_id: source.clone(),
        run_id: run_id.clone(),
        assistant_message_id: "source-assistant".into(),
        cancellation: token.clone(),
        notifications: crate::transport::outbound_channel().0,
        initial_input_id: None,
        current_model_id: Some("fixture-model".into()),
        effective_permissions: AgentPermissions::default(),
    };
    let other_host = StoredWorkflowRuntime {
        service: service.clone(),
        conversation_id: target,
        run_id: "other-run".into(),
        assistant_message_id: "other-assistant".into(),
        cancellation: other_token.clone(),
        notifications: crate::transport::outbound_channel().0,
        initial_input_id: None,
        current_model_id: Some("fixture-model".into()),
        effective_permissions: AgentPermissions::default(),
    };
    let invocation = WorkflowSendInvocation {
        conversation_id: source,
        run_id: run_id.clone(),
        assistant_message_id: "source-assistant".into(),
        tool_call_id: "send-before-stop".into(),
        execution_version: binding.execution_version,
        messages: vec![SendOutput {
            target_node_id: "b".into(),
            message: "Controlled mail".into(),
            reply_to_message_id: None,
        }],
        model_input: None,
        recipient_versions: Default::default(),
    };
    let (writer_tx, writer_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (sending_tx, sending_rx) = mpsc::channel();
    SEND_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(
            run_id.clone(),
            Arc::new(move || {
                sending_tx.send(()).unwrap();
            }),
        );
    let (stop_started_tx, stop_started_rx) = mpsc::channel();
    let (independent_tx, independent_rx) = mpsc::channel();
    let handle = tokio::runtime::Handle::current();
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            storage
                .coordinate_runtime_control(move || {
                    writer_tx.send(()).unwrap();
                    let _ = release_rx.recv_timeout(Duration::from_secs(10));
                    Ok(())
                })
                .unwrap()
        });
        writer_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let send = scope.spawn(|| {
            let _runtime = handle.enter();
            host.send(invocation.clone())
        });
        sending_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let stop = scope.spawn(|| {
            stop_started_tx.send(()).unwrap();
            service.cancel_run_internal(&run_id)
        });
        stop_started_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let independent = scope.spawn(|| {
            let owner = other_host.validate_owner();
            service.register_cancellation("third-run", AgentCancellationToken::default());
            independent_tx.send(owner.is_ok()).unwrap();
            service.cancel_run_internal("other-run")
        });
        let independent_owner = independent_rx.recv_timeout(Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !other_token.is_cancelled() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        let unrelated_stopped = other_token.is_cancelled();
        let same_run_ordered = !token.is_cancelled();
        // Stop A owns no authority until A's exact write completes; B only needs its own gate.
        // Release before assertions so a regression cannot strand the writer or scoped threads.
        release_tx.send(()).unwrap();
        assert!(independent_owner.unwrap());
        assert!(
            unrelated_stopped,
            "unrelated Stop waited for A's writer job"
        );
        assert!(
            same_run_ordered,
            "same-Run Stop crossed the pending mail publication"
        );
        let receipt = send.join().unwrap().unwrap();
        assert_eq!(receipt.messages.len(), 1);
        assert!(stop.join().unwrap().any_effect());
        assert!(independent.join().unwrap().any_effect());
        writer.join().unwrap();
    });
    assert!(token.is_cancelled());
    assert!(host
        .send(WorkflowSendInvocation {
            tool_call_id: "after-stop".into(),
            ..invocation
        })
        .is_err());
    assert_eq!(
        storage
            .workflow_execution_runtime("instance")
            .unwrap()
            .inputs
            .len(),
        1
    );
}
