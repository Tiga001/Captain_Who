use super::provider_transition::{
    conversation_with_completed_history, persist_completed_history, provider_transition_generator,
    two_model_settings,
};
use super::*;
use crate::application::agent::manual_context_compaction::AgentManualContextCompactionOperation;

fn fixture(id: &str) -> (tempfile::TempDir, Arc<StorageService>) {
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    storage
        .save_model_settings(two_model_settings(None))
        .unwrap();
    let mut conversation = conversation_with_completed_history(id, Some("model-1"));
    conversation.messages[0].content =
        "This is a long previous user request with implementation details and retained decisions. "
            .repeat(200);
    let assistant_id = conversation.messages[1].id.clone();
    storage.save_conversation(conversation).unwrap();
    persist_completed_history(&storage, id, &assistant_id);
    (directory, storage)
}

async fn settled(
    service: &AgentService,
    id: &str,
    operation_id: &str,
) -> AgentManualContextCompactionOperation {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let operation = service
                .get_manual_context_compaction_status(AgentManualContextCompactionStatusInput {
                    conversation_id: id.into(),
                    operation_id: Some(operation_id.into()),
                })
                .unwrap()
                .operations
                .pop()
                .unwrap();
            if operation.status != "running" {
                return operation;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn manual_compaction_is_idempotent_preserves_history_and_owns_usage() {
    let id = "manual-complete";
    let (directory, storage) = fixture(id);
    let before = serde_json::to_value(storage.load_conversation(id).unwrap().unwrap()).unwrap();
    let calls = Arc::new(AtomicU64::new(0));
    let call_count = Arc::clone(&calls);
    let generated = provider_transition_generator("model-1");
    let generator: ContextCompactionSummaryGenerator = Arc::new(move |request, cancellation| {
        call_count.fetch_add(1, Ordering::SeqCst);
        let generated = generated.clone();
        Box::pin(async move {
            let mut output = generated(request, cancellation).await?;
            output.observation.actual_usage = Some(mycopilot_core::ModelRequestActualUsage {
                raw: AgentUsage {
                    input_tokens: Some(80),
                    output_tokens: Some(20),
                    total_tokens: Some(100),
                    billable_request_count: Some(1),
                    output_thinking_tokens: None,
                    cached_input_tokens: None,
                    cache_creation_input_tokens: None,
                },
                normalized_input_tokens: Some(80),
                normalization: mycopilot_core::ModelRequestUsageNormalization::OpenAiInputTokens,
            });
            Ok(output)
        })
    });
    let service =
        AgentService::new(storage.clone()).with_context_compaction_summary_generator(generator);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let start = || AgentManualContextCompactionStartInput {
        conversation_id: id.into(),
        request_id: "request-one".into(),
    };
    let running = service
        .start_manual_context_compaction(start(), notifications.clone())
        .unwrap();
    let duplicate = service
        .start_manual_context_compaction(start(), notifications.clone())
        .unwrap();
    assert_eq!(running.operation_id, duplicate.operation_id);
    assert_eq!(running.status, "running");
    let operation = settled(&service, id, &running.operation_id).await;
    assert_eq!(operation.status, "completed", "{:?}", operation.error);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        service
            .cancel_manual_context_compaction(
                AgentManualContextCompactionCancelInput {
                    conversation_id: id.into(),
                    operation_id: running.operation_id.clone()
                },
                notifications.clone()
            )
            .unwrap()
            .status,
        "completed",
        "commit wins over a later cancel"
    );
    assert_eq!(
        serde_json::to_value(storage.load_conversation(id).unwrap().unwrap()).unwrap(),
        before
    );
    let reopened = AgentService::new(storage.clone());
    assert_eq!(
        reopened
            .start_manual_context_compaction(start(), notifications.clone())
            .unwrap()
            .status,
        "completed"
    );
    let noop = reopened
        .start_manual_context_compaction(
            AgentManualContextCompactionStartInput {
                conversation_id: id.into(),
                request_id: "request-two".into(),
            },
            notifications,
        )
        .unwrap();
    assert_eq!(noop.status, "noop");
    let connection = rusqlite::Connection::open(directory.path().join("storage.sqlite")).unwrap();
    let rows: (u64,u64) = connection.query_row("SELECT (SELECT COUNT(*) FROM agent_usage_records), (SELECT COUNT(*) FROM manual_context_compaction_usage_records)",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(rows, (0, 1));
    assert_eq!(
        connection
            .query_row(
                "SELECT total_tokens FROM manual_context_compaction_usage_records",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        100
    );
}

#[tokio::test]
async fn manual_cancel_before_response_prevents_commit_and_keeps_paid_usage() {
    let id = "manual-cancel";
    let (directory, storage) = fixture(id);
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let generated = provider_transition_generator("model-1");
    let ready = entered.clone();
    let unblock = release.clone();
    let generator: ContextCompactionSummaryGenerator = Arc::new(move |request, _cancellation| {
        let ready = ready.clone();
        let unblock = unblock.clone();
        let generated = generated.clone();
        Box::pin(async move {
            ready.notify_one();
            unblock.notified().await;
            generated(request, AgentCancellationToken::new()).await
        })
    });
    let service =
        AgentService::new(storage.clone()).with_context_compaction_summary_generator(generator);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let running = service
        .start_manual_context_compaction(
            AgentManualContextCompactionStartInput {
                conversation_id: id.into(),
                request_id: "cancel-request".into(),
            },
            notifications.clone(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    assert!(service
        .start_manual_context_compaction(
            AgentManualContextCompactionStartInput {
                conversation_id: id.into(),
                request_id: "second-click".into()
            },
            notifications.clone()
        )
        .is_err());
    let mut turn = super::provider_profiles::turn_input("model-1");
    turn.conversation_id = Some(id.into());
    assert!(service
        .start_conversation_turn(turn, notifications.clone())
        .unwrap_err()
        .to_string()
        .contains("正在压缩上下文"));
    let cancelled = service
        .cancel_manual_context_compaction(
            AgentManualContextCompactionCancelInput {
                conversation_id: id.into(),
                operation_id: running.operation_id.clone(),
            },
            notifications.clone(),
        )
        .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert!(cancelled.is_busy);
    assert!(
        service.ensure_no_manual_context_compaction(id).is_err(),
        "draining response must retain admission until usage is durable"
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.ensure_no_manual_context_compaction(id).is_err() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let drained = settled(&service, id, &running.operation_id).await;
    assert_eq!(drained.status, "cancelled");
    assert!(!drained.is_busy);
    let connection = rusqlite::Connection::open(directory.path().join("storage.sqlite")).unwrap();
    let (heads,usage): (u64,u64) = connection.query_row("SELECT (SELECT COUNT(*) FROM conversation_context_compaction_heads), (SELECT COUNT(*) FROM manual_context_compaction_usage_records)",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!((heads, usage), (0, 1));
    assert_eq!(
        service
            .cancel_manual_context_compaction(
                AgentManualContextCompactionCancelInput {
                    conversation_id: id.into(),
                    operation_id: running.operation_id
                },
                notifications
            )
            .unwrap()
            .status,
        "cancelled"
    );
}

#[tokio::test]
async fn manual_failure_keeps_old_context_and_allows_retry_with_new_request() {
    let id = "manual-failure";
    let (_directory, storage) = fixture(id);
    let generator: ContextCompactionSummaryGenerator =
        Arc::new(|_, _| Box::pin(async { Err(AgentError::new("provider failed")) }));
    let service =
        AgentService::new(storage.clone()).with_context_compaction_summary_generator(generator);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let running = service
        .start_manual_context_compaction(
            AgentManualContextCompactionStartInput {
                conversation_id: id.into(),
                request_id: "failed-request".into(),
            },
            notifications.clone(),
        )
        .unwrap();
    assert_eq!(
        settled(&service, id, &running.operation_id).await.status,
        "failed"
    );
    let recovered = AgentService::new(storage.clone())
        .with_context_compaction_summary_generator(provider_transition_generator("model-1"));
    let retry = recovered
        .start_manual_context_compaction(
            AgentManualContextCompactionStartInput {
                conversation_id: id.into(),
                request_id: "retry-request".into(),
            },
            notifications,
        )
        .unwrap();
    assert_eq!(
        settled(&recovered, id, &retry.operation_id).await.status,
        "completed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_compaction_latest_fork_sends_next_turn_with_summary_without_copying_usage() {
    use super::provider_profiles::{
        collect_until_done, read_provider_request, save_provider_profile_fixture, turn_input,
        write_provider_stream,
    };
    use mycopilot_core::storage::models::{ConversationForkPoint, ForkConversationRequest};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_provider_request(&mut stream).await;
        write_provider_stream(
            &mut stream,
            json!({"role":"assistant","content":"The fork continued normally."}),
            "stop",
        )
        .await;
        request
    });
    let id = "manual-latest-fork";
    let (directory, storage) = fixture(id);
    save_provider_profile_fixture(
        &storage,
        &format!("http://{address}/v1/chat/completions"),
        None,
    );
    let service = AgentService::new(storage.clone())
        .with_context_compaction_summary_generator(provider_transition_generator("model-1"));
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let operation = service
        .start_manual_context_compaction(
            AgentManualContextCompactionStartInput {
                conversation_id: id.into(),
                request_id: "latest-compact".into(),
            },
            notifications.clone(),
        )
        .unwrap();
    assert_eq!(
        settled(&service, id, &operation.operation_id).await.status,
        "completed"
    );
    let fork = service
        .fork_conversation_view(ForkConversationRequest {
            request_id: "latest-fork-request".into(),
            source_conversation_id: id.into(),
            fork_point: ConversationForkPoint::Latest {},
        })
        .unwrap();
    let fork_id = fork.conversation.id.clone();
    let connection = rusqlite::Connection::open(directory.path().join("storage.sqlite")).unwrap();
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM manual_context_compaction_usage_records WHERE conversation_id = ?1",[&fork_id],|row|row.get::<_,u64>(0)).unwrap(),0);
    assert_eq!(
        service
            .get_manual_context_compaction_status(AgentManualContextCompactionStatusInput {
                conversation_id: fork_id.clone(),
                operation_id: None
            })
            .unwrap()
            .operations
            .iter()
            .filter(|op| op.status == "completed")
            .count(),
        1
    );
    let mut input = turn_input("model-1");
    input.conversation_id = Some(fork_id.clone());
    input.user_message_id = Some("fork-next-user".into());
    input.assistant_message_id = Some("fork-next-assistant".into());
    input.content = "Continue after manual compaction.".into();
    service
        .start_conversation_turn(input, notifications)
        .unwrap();
    let events = collect_until_done(&mut receiver).await;
    assert_eq!(
        events.last().unwrap()["params"]["status"],
        "completed",
        "{events:?}"
    );
    let request = tokio::time::timeout(Duration::from_secs(5), provider)
        .await
        .unwrap()
        .unwrap();
    let serialized = request.to_string();
    assert!(serialized.contains("已将旧 API 厂商的工具历史压缩为安全摘要"));
    assert!(serialized.contains("Continue after manual compaction."));
    assert!(
        !serialized.contains("This is a long previous user request with implementation details")
    );
    assert_eq!(
        storage
            .load_conversation(&fork_id)
            .unwrap()
            .unwrap()
            .messages
            .last()
            .unwrap()
            .content,
        "The fork continued normally."
    );
}

#[tokio::test]
async fn model_configuration_change_rejects_manual_commit_but_retains_response_usage() {
    let id = "manual-model-stale";
    let (directory, storage) = fixture(id);
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let ready = entered.clone();
    let unblock = release.clone();
    let generated = provider_transition_generator("model-1");
    let generator: ContextCompactionSummaryGenerator = Arc::new(move |request, cancellation| {
        let ready = ready.clone();
        let unblock = unblock.clone();
        let generated = generated.clone();
        Box::pin(async move {
            ready.notify_one();
            unblock.notified().await;
            generated(request, cancellation).await
        })
    });
    let service =
        AgentService::new(storage.clone()).with_context_compaction_summary_generator(generator);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let operation = service
        .start_manual_context_compaction(
            AgentManualContextCompactionStartInput {
                conversation_id: id.into(),
                request_id: "stale-request".into(),
            },
            notifications,
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let mut changed = two_model_settings(None);
    changed.models[0].provider_model_id = "changed-provider-model".into();
    storage.save_model_settings(changed).unwrap();
    release.notify_one();
    let failed = settled(&service, id, &operation.operation_id).await;
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.summary_id, None);
    assert!(service.ensure_no_manual_context_compaction(id).is_ok());
    let connection = rusqlite::Connection::open(directory.path().join("storage.sqlite")).unwrap();
    let (heads,usage,unfinished): (u64,u64,u64) = connection.query_row("SELECT (SELECT COUNT(*) FROM conversation_context_compaction_heads), (SELECT COUNT(*) FROM manual_context_compaction_usage_records), (SELECT COUNT(*) FROM context_compaction_receipts WHERE status='in_progress')",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!((heads, usage, unfinished), (0, 1, 0));
}

#[test]
fn manual_status_bounds_public_history_without_losing_old_operation_lookup() {
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    let mut conversation = conversation_with_completed_history("many-noops", None);
    conversation.messages.clear();
    storage.save_conversation(conversation).unwrap();
    let service = AgentService::new(storage.clone());
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    for index in 0..52 {
        assert_eq!(
            service
                .start_manual_context_compaction(
                    AgentManualContextCompactionStartInput {
                        conversation_id: "many-noops".into(),
                        request_id: format!("noop-{index}"),
                    },
                    notifications.clone()
                )
                .unwrap()
                .status,
            "noop"
        );
    }
    let all = storage
        .list_manual_context_compactions("many-noops")
        .unwrap();
    assert_eq!(all.len(), 52);
    let recent = service
        .get_manual_context_compaction_status(AgentManualContextCompactionStatusInput {
            conversation_id: "many-noops".into(),
            operation_id: None,
        })
        .unwrap();
    assert_eq!(
        recent
            .operations
            .iter()
            .map(|operation| &operation.operation_id)
            .collect::<Vec<_>>(),
        all[2..]
            .iter()
            .map(|operation| &operation.operation_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        service
            .get_manual_context_compaction_status(AgentManualContextCompactionStatusInput {
                conversation_id: "many-noops".into(),
                operation_id: Some(all[0].operation_id.clone()),
            })
            .unwrap()
            .operations
            .len(),
        1
    );
}
