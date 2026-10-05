//! Continuous sends through real admission, initial publication and terminal observers.
//! A correct helper-only cache hit is insufficient if any later observer rebuilds the history.
#![cfg(debug_assertions)] // Work counters are intentionally absent from production builds.
use super::*;
use mycopilot_core::storage::models::ChatConversationMetaRecord;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const ID: &str = "continuous-warm-send";

fn input(content: &str) -> AgentConversationTurnInput {
    AgentConversationTurnInput {
        conversation_id: Some(ID.into()),
        project_id: None,
        model_id: "model-1".into(),
        context_window_indicator_enabled: true,
        content: content.into(),
        attachments: vec![],
        folder_references: vec![],
        skills: vec![],
        title: None,
        user_message_id: None,
        assistant_message_id: None,
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
    }
}

fn preview(service: &AgentService) {
    service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: Some(ID.into()),
            project_id: None,
            model_id: "model-1".into(),
            max_tokens: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: vec![],
        })
        .unwrap();
}

async fn accept_request(listener: &TcpListener) -> (TcpStream, Value) {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "local provider received a complete HTTP request");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let length = String::from_utf8_lossy(&bytes[..end])
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    let request =
                        serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                    return (stream, request);
                }
            }
        }
    })
    .await
    .expect("a warm turn reaches the local provider")
}

async fn reply(stream: &mut TcpStream, content: &str) {
    let delta =
        json!({"choices":[{"delta":{"role":"assistant","content":content},"finish_reason":null}]});
    let end = json!({"choices":[{"delta":{},"finish_reason":"stop"}]});
    stream
        .write_all(
            format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {delta}\n\ndata: {end}\n\ndata: [DONE]\n\n")
                .as_bytes(),
        )
        .await
        .unwrap();
}

async fn wait_for_terminal(
    service: &AgentService,
    notifications: &mut crate::transport::OutboundReceiver,
    assistant_message_id: &str,
) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(event) = notifications.recv().await {
            assert_ne!(event["params"]["type"], "error", "{event}");
            if event["params"]["type"] == "done" {
                break;
            }
        }
        while service.has_conversation_turn_occupancy(ID).unwrap() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the local provider turn settles");
    let trace = service
        .storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
}

fn occurrences_in_messages(request: &Value, text: &str) -> usize {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["content"].to_string().contains(text))
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn continuous_warm_send_survives_metadata_and_observers_without_full_history_rebuild() {
    let directory = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&directory.path().join("continuous.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: ID.into(),
            project_id: None,
            model_id: Some("model-1".into()),
            title: "Continuous send".into(),
            messages: vec![],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = crate::transport::outbound_channel();
    let first = service
        .start_conversation_turn(input("First user request 627391"), notifications.clone())
        .unwrap();
    let (mut first_stream, first_request) = accept_request(&listener).await;
    assert_eq!(
        occurrences_in_messages(&first_request, "First user request 627391"),
        1
    );
    reply(&mut first_stream, "First short reply 892146").await;
    drop(first_stream);
    wait_for_terminal(&service, &mut events, &first.assistant_message_id).await;

    // The real terminal observer and backend warmup must publish this baseline. Calling an
    // artificial warm_preview here would hide the missing post-terminal measured baseline.
    tokio::time::timeout(Duration::from_secs(15), async {
        while !service.has_measured_prepared_history_for_test(ID) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("settled real turns preserve a measured next-send prefix");

    let settled = storage.load_conversation(ID).unwrap().unwrap();
    let previous_revision = storage.load_conversation_for_turn(ID).unwrap().1.unwrap();
    let before_metadata_storage = storage.history_snapshot_diagnostics(ID);
    let before_metadata_server = service.prepared_history_diagnostics_for_test(ID);
    assert!(before_metadata_storage.full_loads > 0);
    assert!(before_metadata_storage.trace_decodes > 0);
    assert!(before_metadata_storage.model_context_decodes > 0);
    assert!(before_metadata_server.history_assemblies > 0);
    assert!(before_metadata_server.context_rebuilds > 0);
    storage
        .save_conversation_meta(ChatConversationMetaRecord {
            id: settled.id,
            project_id: settled.project_id,
            model_id: settled.model_id,
            title: "Read and pinned between sends".into(),
            created_at: settled.created_at,
            updated_at: settled.updated_at + 1,
            pinned_at: Some(settled.updated_at + 1),
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    assert!(storage.load_conversation_for_turn(ID).unwrap().1.unwrap() > previous_revision);

    // The current renderer can ask for a preview while terminal or idle preparation is finishing.
    // Concurrent readers must neither decode the same stable history again nor erase measurement.
    let gate = Arc::new(std::sync::Barrier::new(6));
    let mut readers = Vec::new();
    for index in 0..6 {
        let service = service.clone();
        let gate = gate.clone();
        readers.push(tokio::task::spawn_blocking(move || {
            gate.wait();
            if index % 2 == 0 {
                preview(&service);
            } else {
                service.prepare_cached_history(ID).unwrap().unwrap();
            }
        }));
    }
    for reader in readers {
        reader.await.unwrap();
    }
    assert!(service.has_measured_prepared_history_for_test(ID));
    let before_send_storage = storage.history_snapshot_diagnostics(ID);
    let before_send_server = service.prepared_history_diagnostics_for_test(ID);
    assert_eq!(
        before_send_storage.full_loads,
        before_metadata_storage.full_loads
    );
    assert_eq!(
        before_send_storage.trace_decodes,
        before_metadata_storage.trace_decodes
    );
    assert_eq!(
        before_send_storage.model_context_decodes,
        before_metadata_storage.model_context_decodes
    );
    assert_eq!(
        before_send_server.history_assemblies,
        before_metadata_server.history_assemblies
    );
    assert_eq!(
        before_send_server.context_rebuilds,
        before_metadata_server.context_rebuilds
    );

    let second = service
        .start_conversation_turn(input("Second user request 563207"), notifications)
        .unwrap();
    let (mut second_stream, second_request) = accept_request(&listener).await;

    // Receiving the HTTP request proves setup and the first real publication observer ran.
    // Inspect actual work counters before replying, so terminal maintenance is not conflated
    // with the user-visible send path. A cache boolean alone cannot prove this optimization.
    let after_send_storage = storage.history_snapshot_diagnostics(ID);
    let after_send_server = service.prepared_history_diagnostics_for_test(ID);
    assert_eq!(
        after_send_storage.full_loads,
        before_send_storage.full_loads
    );
    assert_eq!(
        after_send_storage.trace_decodes,
        before_send_storage.trace_decodes
    );
    assert_eq!(
        after_send_storage.model_context_decodes,
        before_send_storage.model_context_decodes
    );
    assert_eq!(
        after_send_server.history_assemblies,
        before_send_server.history_assemblies
    );
    assert_eq!(
        after_send_server.context_rebuilds,
        before_send_server.context_rebuilds
    );
    assert_eq!(
        after_send_server.measured_installs,
        before_send_server.measured_installs + 1
    );
    {
        let states = service.conversation_context_states.lock().unwrap();
        let state = states.get(ID).unwrap();
        assert_eq!(state.active_run_id.as_deref(), Some(second.run_id.as_str()));
        assert!(!state.awaiting_initial_publication);
        assert!(state.committed_activity_items > 0);
    }
    assert_eq!(
        occurrences_in_messages(&second_request, "First user request 627391"),
        1
    );
    assert_eq!(
        occurrences_in_messages(&second_request, "First short reply 892146"),
        1
    );
    assert_eq!(
        occurrences_in_messages(&second_request, "Second user request 563207"),
        1
    );
    let initial_trace = storage
        .get_conversation_turn_trace(&second.assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        initial_trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ContextMaterial {
                    material_kind: mycopilot_core::ConversationContextMaterialKind::RunWorldState,
                    ..
                }
            ))
            .count(),
        1,
        "the current run bootstrap is published once"
    );
    reply(&mut second_stream, "Second short reply 941830").await;
    drop(second_stream);
    wait_for_terminal(&service, &mut events, &second.assistant_message_id).await;
    assert_eq!(storage.list_conversation_turn_traces(ID).unwrap().len(), 2);
}
