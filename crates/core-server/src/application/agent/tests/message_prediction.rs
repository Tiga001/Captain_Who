use super::provider_profiles::{collect_until_done, read_provider_request, write_provider_stream};
use super::*;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

async fn request(listener: &TcpListener) -> (TcpStream, Value) {
    tokio::time::timeout(Duration::from_secs(12), async {
        let (mut stream, _) = listener.accept().await.unwrap();
        let body = read_provider_request(&mut stream).await;
        (stream, body)
    })
    .await
    .expect("one prediction request reached the local fixture")
}

async fn reply(mut stream: TcpStream, body: &Value, content: &str) {
    if body["stream"] == true {
        write_provider_stream(
            &mut stream,
            json!({"role":"assistant","content":content}),
            "stop",
        )
        .await;
    } else {
        let json = json!({"choices":[{"message":{"role":"assistant","content":content},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":12,"completion_tokens":4,"total_tokens":16}}).to_string();
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",json.len()).as_bytes()).await.unwrap();
    }
}

fn prediction(
    service: &AgentService,
    input: &PromptPredictionInput,
) -> tokio::task::JoinHandle<Result<Option<String>, AgentServiceError>> {
    let service = service.clone();
    let input = input.clone();
    let access = service.capture_execution_access().unwrap();
    tokio::spawn(async move { service.predict_next_user_message(input, access).await })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_prediction_inherits_live_prefix_coalesces_and_restores_without_writing_a_turn() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("composer.sqlite");
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let storage =
        Arc::new(StorageService::open_with_model_credentials(&path, credentials.clone()).unwrap());
    let provider = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        provider.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    storage.set_prediction_preference(true).unwrap();
    let host = AgentService::new_authorized_for_test(storage.clone());
    let (notify, mut events) = crate::transport::outbound_channel();
    let started = host.start_conversation_turn(serde_json::from_value(json!({
        "modelId":"model-1", "content":"Does our profile need an update?", "skills":[], "attachments":[],
        "permissions": AgentPermissions::default()
    })).unwrap(), notify).unwrap();
    let (stream, parent) = request(&provider).await;
    reply(stream, &parent, "The existing profile is current.").await;
    collect_until_done(&mut events).await;
    let target = PromptPredictionInput {
        conversation_id: started.conversation_id.clone(),
        assistant_message_id: started.assistant_message_id,
        model_id: "model-1".into(),
    };
    let version = storage
        .current_conversation_history_version(&target.conversation_id)
        .unwrap()
        .unwrap();
    let live = prediction(&host, &target);
    let (stream, sample) = request(&provider).await;
    let concurrent = prediction(&host, &target);
    let parent_messages = parent["messages"].as_array().unwrap();
    let sample_messages = sample["messages"].as_array().unwrap();
    assert_eq!(&sample_messages[..parent_messages.len()], parent_messages);
    assert_eq!(sample["tools"], parent["tools"]);
    assert_eq!(sample_messages.len(), parent_messages.len() + 2);
    assert_eq!(
        sample_messages[parent_messages.len()]["content"],
        "The existing profile is current."
    );
    reply(
        stream,
        &sample,
        r#"{"suggestion":"Keep the existing profile."}"#,
    )
    .await;
    for result in [live, concurrent, prediction(&host, &target)] {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), result)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .as_deref(),
            Some("Keep the existing profile.")
        );
    }
    assert!(storage
        .is_conversation_history_snapshot_current(&version)
        .unwrap());
    assert_eq!(
        storage
            .load_conversation(&target.conversation_id)
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2
    );
    assert!(storage
        .load_composer_draft(&target.conversation_id)
        .unwrap()
        .is_none());

    // Reopen SQLite with the same credential backend and completely new in-memory caches.
    drop(host);
    drop(storage);
    let storage =
        Arc::new(StorageService::open_with_model_credentials(&path, credentials).unwrap());
    let version = storage
        .current_conversation_history_version(&target.conversation_id)
        .unwrap()
        .unwrap();
    let restarted = AgentService::new_authorized_for_test(storage.clone());
    let cold = prediction(&restarted, &target);
    let (stream, body) = request(&provider).await;
    let cold_messages = body["messages"].as_array().unwrap();
    assert!(cold_messages.iter().any(|m| m["role"] == "user"
        && m["content"]
            .as_str()
            .is_some_and(|s| s.contains("Does our profile"))));
    assert!(cold_messages.iter().any(|m| m["role"] == "assistant"
        && m["content"]
            .as_str()
            .is_some_and(|s| s.contains("profile is current"))));
    reply(stream, &body, r#"{"suggestion":null}"#).await;
    assert_eq!(cold.await.unwrap().unwrap(), None);
    assert_eq!(
        prediction(&restarted, &target).await.unwrap().unwrap(),
        None
    );
    assert!(storage
        .is_conversation_history_snapshot_current(&version)
        .unwrap());

    // Disabling while a fresh prediction is actually on the wire must retire its result, yet
    // preserve the usage of the already-dispatched request.
    restarted.clear_message_predictions();
    let outstanding = prediction(&restarted, &target);
    let (_stream, _) = request(&provider).await;
    storage.set_prediction_preference(false).unwrap();
    restarted.clear_message_predictions();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), outstanding)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        None
    );
    assert_eq!(
        prediction(&restarted, &target).await.unwrap().unwrap(),
        None
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), provider.accept())
            .await
            .is_err()
    );
    let db = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM message_prediction_usage", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        3
    );
}
