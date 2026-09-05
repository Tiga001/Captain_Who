use super::*;

use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
use crate::provider_profile::{
    ProviderFamilySettings, ProviderProfileRef, ProviderReasoningEffort, ProviderVendorId,
};
use crate::storage::models::{AgentRunGuidanceRecord, ChatConversationRecord, ChatMessageRecord};
use crate::{ProviderContinuationVaultFactory, ProviderProfileConfig};
use std::sync::atomic::{AtomicI64, Ordering};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const STEER_REASONING: &str = "moonshot-private-steer-reasoning-canary";
const FINAL_REASONING: &str = "moonshot-private-final-reasoning-canary";
const INTERRUPTED_REASONING: &str = "moonshot-private-interrupted-reasoning-canary";
const RECOVERED_REASONING: &str = "moonshot-private-recovered-reasoning-canary";
const STEER_VISIBLE: &str = "Visible answer before the steer boundary.";
const FINAL_VISIBLE: &str = "Visible final answer after the steer boundary.";

#[derive(Clone, Copy)]
enum MoonshotRecoveryFamily {
    K3,
    K27Code,
}

impl MoonshotRecoveryFamily {
    const fn slug(self) -> &'static str {
        match self {
            Self::K3 => "k3",
            Self::K27Code => "k2-7-code",
        }
    }

    const fn model_id(self) -> &'static str {
        match self {
            Self::K3 => "kimi-k3",
            Self::K27Code => "kimi-k2.7-code",
        }
    }

    const fn profile(self) -> ProviderProfileConfig {
        match self {
            Self::K3 => ProviderProfileConfig::from_family_settings(
                ProviderProfileRef::moonshot_k3_chat(),
                ProviderVendorId::Moonshot,
                ProviderFamilySettings::MoonshotK3Chat {
                    reasoning_effort: ProviderReasoningEffort::Max,
                },
            ),
            Self::K27Code => ProviderProfileConfig::from_family_settings(
                ProviderProfileRef::moonshot_k2_7_code_chat(),
                ProviderVendorId::Moonshot,
                ProviderFamilySettings::MoonshotK27CodeChat,
            ),
        }
    }
}

fn pending_assistant(id: &str, created_at: i64) -> ChatMessageRecord {
    ChatMessageRecord {
        human_interaction_response: None,
        id: id.to_string(),
        role: "assistant".to_string(),
        content: String::new(),
        created_at,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    }
}

fn moonshot_runtime_input(
    api_url: &str,
    profile: ProviderProfileConfig,
    protocol: ProviderProtocolKey,
    model_id: &str,
    assistant_message_id: &str,
    conversation_id: &str,
    messages: Vec<AgentChatMessage>,
) -> AgentChatInput {
    let mut input = conversation_context_input(messages);
    input.api_url = api_url.to_string();
    input.api_token = "moonshot-runtime-test-token".to_string();
    input.provider_profile_config = Some(profile);
    input.provider_protocol_key = Some(protocol);
    input.model = model_id.to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some(assistant_message_id.to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    input
}

fn count_occurrences(value: &Value, needle: &str) -> usize {
    serde_json::to_string(value)
        .unwrap()
        .matches(needle)
        .count()
}

fn assert_request_private_replay(
    request: &Value,
    expected_steer: usize,
    expected_final: usize,
    expected_interrupted: usize,
) {
    assert_eq!(count_occurrences(request, STEER_REASONING), expected_steer);
    assert_eq!(count_occurrences(request, FINAL_REASONING), expected_final);
    assert_eq!(
        count_occurrences(request, INTERRUPTED_REASONING),
        expected_interrupted
    );
}

fn assert_empty_turns_preserve_terminal_audit(request: &Value) {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .expect("restart request must contain messages");
    let steer_index = messages
        .iter()
        .position(|message| {
            message.get("reasoning_content").and_then(Value::as_str) == Some(STEER_REASONING)
        })
        .expect("restart request must restore the empty steer turn");
    let final_index = messages
        .iter()
        .position(|message| {
            message.get("reasoning_content").and_then(Value::as_str) == Some(FINAL_REASONING)
        })
        .expect("restart request must restore the empty final turn");
    let guidance_index = messages
        .iter()
        .position(|message| {
            message.get("role").and_then(Value::as_str) == Some("user")
                && message.get("content").and_then(Value::as_str)
                    == Some("Apply the durable steer after this response.")
        })
        .expect("restart request must retain the exact durable guidance boundary");
    let terminal_audit_index = messages
        .iter()
        .position(|message| {
            message
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|content| {
                    content.contains("Historical agent activity terminal record")
                })
        })
        .expect("restart request must retain the synthetic terminal audit owner");

    assert_eq!(messages[steer_index].get("content"), Some(&json!("")));
    assert_eq!(messages[final_index].get("content"), Some(&json!("")));
    assert_eq!(steer_index.checked_add(1), Some(guidance_index));
    assert!(guidance_index < final_index);
    assert!(final_index < terminal_audit_index);
    assert!(messages[terminal_audit_index]
        .get("reasoning_content")
        .is_none());
}

fn durable_assistant_history(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
) -> AgentChatMessage {
    let conversation = storage
        .load_conversation(conversation_id)
        .unwrap()
        .expect("restart must reload the durable conversation");
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == assistant_message_id)
        .expect("restart must reload the terminal assistant message");
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .expect("restart must reload the terminal trace");
    let model_context_items = storage
        .get_conversation_model_context_log(assistant_message_id)
        .unwrap()
        .expect("restart must reload the exact model-context projection")
        .items;
    AgentChatMessage {
        message_id: Some(assistant_message_id.to_string()),
        role: "assistant".to_string(),
        content: assistant.content.clone(),
        created_at: Some(assistant.created_at),
        conversation_turn_trace: Some(trace),
        conversation_model_context_items: model_context_items,
    }
}

async fn assert_moonshot_ordinary_recovery(family: MoonshotRecoveryFamily, empty_visible: bool) {
    let slug = if empty_visible {
        format!("{}-empty", family.slug())
    } else {
        family.slug().to_string()
    };
    let conversation_id = format!("conversation-moonshot-{slug}-ordinary-recovery");
    let first_assistant_id = format!("assistant-moonshot-{slug}-first");
    let interrupted_assistant_id = format!("assistant-moonshot-{slug}-interrupted");
    let recovered_assistant_id = format!("assistant-moonshot-{slug}-recovered");
    let first_run_id = format!("run-moonshot-{slug}-first");
    let interrupted_run_id = format!("run-moonshot-{slug}-interrupted");
    let recovered_run_id = format!("run-moonshot-{slug}-recovered");
    let guidance_id = format!("guidance-moonshot-{slug}");
    let client_message_id = format!("client-moonshot-{slug}");

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("runtime.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.clone(),
            project_id: None,
            model_id: Some(family.model_id().to_string()),
            title: format!("Moonshot {slug} ordinary recovery"),
            messages: vec![
                pending_assistant(&first_assistant_id, 1),
                pending_assistant(&interrupted_assistant_id, 2),
                pending_assistant(&recovered_assistant_id, 3),
            ],
            created_at: 1,
            updated_at: 3,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .store_agent_run_guidance(AgentRunGuidanceRecord {
            guidance_id: guidance_id.clone(),
            client_message_id: client_message_id.clone(),
            run_id: first_run_id.clone(),
            conversation_id: conversation_id.clone(),
            assistant_message_id: first_assistant_id.clone(),
            content: "Apply the durable steer after this response.".to_string(),
            status: crate::AgentGuidanceStatus::Queued,
            attachment_ids: Vec::new(),
            applied_trace_sequence: None,
            terminal_reason: None,
            created_at: 4,
            updated_at: 4,
        })
        .unwrap();

    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&storage),
            Arc::clone(&credentials),
        )
        .unwrap(),
    );
    let profile = family.profile();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        family.model_id(),
        None,
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let api_url = format!("http://{address}/v1/chat/completions");
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for request_index in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            if request_index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
            }
            let (content, reasoning_content) = match request_index {
                0 => (
                    if empty_visible { "" } else { STEER_VISIBLE },
                    STEER_REASONING,
                ),
                1 => (
                    if empty_visible { "" } else { FINAL_VISIBLE },
                    FINAL_REASONING,
                ),
                2 => (
                    "This response is interrupted before Host terminal commit.",
                    INTERRUPTED_REASONING,
                ),
                _ => (
                    "Recovered without the interrupted stage.",
                    RECOVERED_REASONING,
                ),
            };
            write_runtime_test_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": content,
                            "reasoning_content": reasoning_content,
                        },
                        "finish_reason": "stop",
                    }]
                }),
            )
            .await;
        }
    });

    let snapshots = Arc::new(Mutex::new(Vec::<ConversationTraceSnapshot>::new()));
    let snapshots_for_observer = Arc::clone(&snapshots);
    let storage_for_observer = Arc::clone(&storage);
    let observer_run_id = first_run_id.clone();
    let observer_conversation_id = conversation_id.clone();
    let observer_assistant_id = first_assistant_id.clone();
    let observer_timestamp = Arc::new(AtomicI64::new(10));
    let observer_timestamp_for_callback = Arc::clone(&observer_timestamp);
    let trace_observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        let updated_at = observer_timestamp_for_callback.fetch_add(1, Ordering::SeqCst);
        let trace = snapshot.in_progress_audit_trace(
            &observer_run_id,
            &observer_conversation_id,
            &observer_assistant_id,
        );
        storage_for_observer
            .append_in_progress_conversation_turn_trace_and_apply_guidances(
                &trace,
                &snapshot.model_context_items,
                1,
                updated_at,
            )
            .map_err(AgentError::new)?;
        snapshots_for_observer.lock().unwrap().push(snapshot);
        Ok(None)
    });

    let queue = AgentSteerInputQueue::new();
    let first_input = moonshot_runtime_input(
        &api_url,
        profile.clone(),
        protocol.clone(),
        family.model_id(),
        &first_assistant_id,
        &conversation_id,
        vec![message(
            "user",
            "Start the Moonshot preserved-thinking task.",
        )],
    );
    let runtime_queue = queue.clone();
    let runtime_vault = Arc::clone(&vault);
    let runtime_storage = Arc::clone(&storage);
    let runtime_run_id = first_run_id.clone();
    let first_runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                first_input,
                Some(runtime_run_id),
                None,
                AgentCancellationToken::new(),
                Some(
                    AgentRuntimeHostServices::new()
                        .with_storage(runtime_storage)
                        .with_provider_continuation_vault(runtime_vault)
                        .with_trace_observer(trace_observer)
                        .with_steer_input(runtime_queue),
                ),
            )
            .await
            .unwrap()
    });
    first_request_seen_rx.await.unwrap();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                &guidance_id,
                &client_message_id,
                "Apply the durable steer after this response.",
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    release_first_response_tx.send(()).unwrap();
    let first_output = first_runtime.await.unwrap();
    assert_eq!(first_output.status, AgentRunStatus::Completed);
    assert_eq!(
        first_output.content,
        if empty_visible { "" } else { FINAL_VISIBLE }
    );

    let terminal_trace = first_output
        .conversation_turn_trace
        .clone()
        .expect("runtime must return the terminal trace");
    let last_snapshot = snapshots
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("durable observer must receive the steer projection");
    storage
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            &conversation_id,
            &first_assistant_id,
            &first_output.content,
            Some("sent"),
            "completed",
            &terminal_trace,
            Some(&last_snapshot.model_context_items),
            1,
            100,
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        vault
            .list_replayable_for_conversation(&conversation_id, &protocol)
            .unwrap()
            .len(),
        2,
        "the steer narration and final ordinary turn must both be active"
    );
    for public_projection in [
        serde_json::to_string(&first_output).unwrap(),
        serde_json::to_string(&terminal_trace).unwrap(),
        serde_json::to_string(&last_snapshot.items).unwrap(),
        serde_json::to_string(&last_snapshot.model_context_items).unwrap(),
    ] {
        assert!(!public_projection.contains(STEER_REASONING));
        assert!(!public_projection.contains(FINAL_REASONING));
        assert!(!public_projection.contains("providerContinuation"));
    }

    drop(vault);
    drop(storage);

    // First restart: the next HTTP payload must restore both exact ordinary continuations from
    // durable trace/message projections. Its own final continuation is deliberately not committed,
    // simulating process interruption after Runtime staging and before Host terminal visibility.
    let restarted_storage = Arc::new(StorageService::open(&database_path).unwrap());
    let restarted_vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&restarted_storage),
            Arc::clone(&credentials),
        )
        .unwrap(),
    );
    let durable_history =
        durable_assistant_history(&restarted_storage, &conversation_id, &first_assistant_id);
    let interrupted_input = moonshot_runtime_input(
        &api_url,
        profile.clone(),
        protocol.clone(),
        family.model_id(),
        &interrupted_assistant_id,
        &conversation_id,
        vec![
            message("user", "Start the Moonshot preserved-thinking task."),
            durable_history.clone(),
            message("user", "Continue after the application restart."),
        ],
    );
    let interrupted_output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            interrupted_input,
            Some(interrupted_run_id),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(Arc::clone(&restarted_storage))
                    .with_provider_continuation_vault(Arc::clone(&restarted_vault)),
            ),
        )
        .await
        .unwrap();
    assert_eq!(interrupted_output.status, AgentRunStatus::Completed);
    assert_eq!(
        restarted_vault
            .list_replayable_for_conversation(&conversation_id, &protocol)
            .unwrap()
            .len(),
        2,
        "an uncommitted final turn must remain staged and invisible"
    );
    drop(restarted_vault);
    drop(restarted_storage);

    // Second restart: the interrupted staged envelope must still be excluded from replay.
    let recovered_storage = Arc::new(StorageService::open(&database_path).unwrap());
    let recovered_vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&recovered_storage),
            Arc::clone(&credentials),
        )
        .unwrap(),
    );
    assert_eq!(
        recovered_vault
            .list_replayable_for_conversation(&conversation_id, &protocol)
            .unwrap()
            .len(),
        2
    );
    let recovered_history =
        durable_assistant_history(&recovered_storage, &conversation_id, &first_assistant_id);
    let recovered_input = moonshot_runtime_input(
        &api_url,
        profile,
        protocol,
        family.model_id(),
        &recovered_assistant_id,
        &conversation_id,
        vec![
            message("user", "Start the Moonshot preserved-thinking task."),
            recovered_history,
            message("user", "Retry after the interrupted terminal commit."),
        ],
    );
    let recovered_output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            recovered_input,
            Some(recovered_run_id),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(recovered_storage)
                    .with_provider_continuation_vault(recovered_vault),
            ),
        )
        .await
        .unwrap();
    assert_eq!(recovered_output.status, AgentRunStatus::Completed);
    assert_eq!(
        recovered_output.content,
        "Recovered without the interrupted stage."
    );

    server.await.unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_request_private_replay(&requests[0], 0, 0, 0);
    assert_request_private_replay(&requests[1], 1, 0, 0);
    assert_request_private_replay(&requests[2], 1, 1, 0);
    assert_request_private_replay(&requests[3], 1, 1, 0);
    if empty_visible {
        assert_empty_turns_preserve_terminal_audit(&requests[2]);
        assert_empty_turns_preserve_terminal_audit(&requests[3]);
    } else {
        assert!(serde_json::to_string(&requests[2])
            .unwrap()
            .contains(STEER_VISIBLE));
        assert!(serde_json::to_string(&requests[2])
            .unwrap()
            .contains(FINAL_VISIBLE));
    }
    assert!(!serde_json::to_string(&requests[3])
        .unwrap()
        .contains(INTERRUPTED_REASONING));
}

#[tokio::test]
async fn moonshot_k3_preserved_thinking_survives_restart_and_ignores_interrupted_stage() {
    assert_moonshot_ordinary_recovery(MoonshotRecoveryFamily::K3, false).await;
}

#[tokio::test]
async fn moonshot_k2_7_preserved_thinking_survives_restart_and_ignores_interrupted_stage() {
    assert_moonshot_ordinary_recovery(MoonshotRecoveryFamily::K27Code, false).await;
}

#[tokio::test]
async fn moonshot_k3_empty_preserved_thinking_survives_steer_terminal_and_restart() {
    assert_moonshot_ordinary_recovery(MoonshotRecoveryFamily::K3, true).await;
}

#[tokio::test]
async fn moonshot_k2_7_empty_preserved_thinking_survives_steer_terminal_and_restart() {
    assert_moonshot_ordinary_recovery(MoonshotRecoveryFamily::K27Code, true).await;
}
