//! Characterize the actual HTTP prefix across read-file -> next-turn thanks.
//! No context repair here: include the Renderer persistence ordering that a
//! direct Harness fixture omits, and report the first changed wire message.
use super::provider_profiles::{collect_until_done, read_provider_request, write_provider_stream};
use super::*;
use mycopilot_core::{fingerprint_llm_request, LlmRequestFingerprint};
use tokio::net::TcpListener;
use tokio::sync::mpsc::unbounded_channel;

const CONVERSATION: &str = "wire-prefix-conversation";
const PROJECT: &str = "wire-prefix-project";
const TASK: &str = "读取工作区文件，说明这是什么项目";

#[derive(Clone, Copy, Debug)]
enum OptimisticSave {
    Absent,
    AfterFirstRequest,
}

fn turn_input(model: &str, index: usize) -> AgentConversationTurnInput {
    let mut input = super::provider_profiles::turn_input(model);
    input.conversation_id = Some(CONVERSATION.into());
    input.project_id = Some(PROJECT.into());
    input.content = if index == 0 { TASK } else { "谢谢" }.into();
    input.user_message_id = Some(format!("wire-user-{index}"));
    input.assistant_message_id = Some(format!("wire-assistant-{index}"));
    input
}

fn optimistic_pair(timestamp: i64) -> Vec<ChatMessageRecord> {
    ["user", "assistant"]
        .into_iter()
        .map(|role| ChatMessageRecord {
            human_interaction_response: None,
            id: format!("wire-{role}-0"),
            role: role.into(),
            content: if role == "user" { TASK } else { "" }.into(),
            created_at: timestamp,
            status: Some(if role == "user" { "sent" } else { "pending" }.into()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        })
        .collect()
}

async fn wait_for_worker_release(service: &AgentService, run_id: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while service.cancellations.lock().unwrap().contains_key(run_id) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("completed Host worker must release before the next turn");
}

async fn capture_read_then_thanks(save: OptimisticSave) -> Vec<Value> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (captured, mut requests) = unbounded_channel();
    // Hold the first Provider response until the selected optimistic-save order
    // has happened. This models slow IPC persistence without a timing-based race.
    let (release, released) = tokio::sync::oneshot::channel();
    let provider = tokio::spawn(async move {
        let mut released = Some(released);
        let responses = [
            (
                json!({"role":"assistant", "content":"先查看目录。", "reasoning_content":"Inspect the workspace before reading files.", "tool_calls":[{"index":0,"id":"call_map","type":"function","function":{"name":"workspace_map","arguments":"{\"maxDepth\":1,\"maxEntries\":20}"}}]}),
                "tool_calls",
            ),
            (
                json!({"role":"assistant", "content":"再读取两个文件。", "reasoning_content":"Read both files in one complete tool batch.", "tool_calls":[{"index":0,"id":"call_read_readme","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"README.md\"}"}},{"index":1,"id":"call_read_package","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"package.json\"}"}}]}),
                "tool_calls",
            ),
            (
                json!({"role":"assistant", "content":"这是一个桌面助手项目。", "reasoning_content":"The two files confirm the project description."}),
                "stop",
            ),
            (
                json!({"role":"assistant", "content":"不客气。", "reasoning_content":"Acknowledge thanks."}),
                "stop",
            ),
        ];
        for (index, (delta, finish)) in responses.into_iter().enumerate() {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(20), listener.accept())
                .await
                .unwrap()
                .unwrap();
            captured
                .send(read_provider_request(&mut stream).await)
                .unwrap();
            if index == 0 {
                released.take().unwrap().await.unwrap();
            }
            write_provider_stream(&mut stream, delta, finish).await;
        }
    });

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("README.md"),
        "# Wire fixture\nA desktop assistant.\n".repeat(100),
    )
    .unwrap();
    fs::write(
        workspace.join("package.json"),
        "{\"name\":\"wire-fixture\",\"private\":true}",
    )
    .unwrap();
    let storage = Arc::new(
        StorageService::open_with_model_credentials(
            &fixture.path().join("storage.sqlite"),
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    // Match the observed native DeepSeek profile, including reasoning replay.
    // Only this loopback endpoint and fake credentials are used.
    let settings = storage.save_model_settings_request(serde_json::from_value(json!({
        "expectedRevision":null,
        "apiUrl":format!("http://{address}/v1/chat/completions"),
        "apiTokenMutation":{"type":"replace","value":"FAKE_WIRE_FIXTURE_TOKEN"},
        "searchMode":"tavily",
        "tavilyApiKeyMutation":{"type":"replace","value":"FAKE_SEARCH_TOKEN"},
        "models":[{
            "id":null,"providerModelId":"deepseek-v4-flash","displayName":"Wire fixture",
            "apiUrlOverride":null,"apiTokenOverrideMutation":{"type":"clear"},
            "supportsImage":false,"contextWindowTokens":128000,
            "providerProfileUpdate":{"kind":"select_registered_profile","profileId":"deepseek_v4_chat",
                "settings":{"kind":"deepseek_v4_chat","reasoning":{"mode":"enabled","effort":"high"}}},
            "inputPrice":"0","cachedInputPrice":"","outputPrice":"0","enabled":true
        }]
    })).unwrap()).unwrap();
    let model = &settings.models[0].id;
    storage
        .save_project(ProjectRecord {
            id: PROJECT.into(),
            name: "Wire workspace".into(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION.into(),
            project_id: Some(PROJECT.into()),
            model_id: Some(model.clone()),
            title: TASK.into(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    // Same-ms optimistic user/assistant, generated before the Host prepares its
    // own authoritative pair. Deliberately distinct, not relying on clock speed.
    let optimistic_timestamp = 1_788_713_110_306;
    let (notifications, mut events) = unbounded_channel();
    let first = service
        .start_conversation_turn(turn_input(model, 0), notifications.clone())
        .unwrap();
    let mut wires = vec![
        tokio::time::timeout(Duration::from_secs(10), requests.recv())
            .await
            .unwrap()
            .unwrap(),
    ];
    assert_ne!(first.user_message.created_at, optimistic_timestamp);
    if matches!(save, OptimisticSave::AfterFirstRequest) {
        // Same persistence API as storage.upsertChatMessages; does not reassemble
        // the live Run. Its frozen message time and the DB can now disagree.
        storage
            .upsert_chat_messages(CONVERSATION, optimistic_pair(optimistic_timestamp), 0)
            .unwrap();
    }
    release.send(()).unwrap();
    let first_events = collect_until_done(&mut events).await;
    assert_eq!(
        first_events.last().unwrap()["params"]["status"],
        "completed",
        "{first_events:?}"
    );
    wait_for_worker_release(&service, &first.run_id).await;
    for _ in 0..2 {
        wires.push(requests.recv().await.unwrap());
    }
    let saved = storage.load_conversation(CONVERSATION).unwrap().unwrap();
    for (id, authoritative_time) in [
        (&first.user_message_id, first.user_message.created_at),
        (
            &first.assistant_message_id,
            first.assistant_message.created_at,
        ),
    ] {
        assert_eq!(
            saved
                .messages
                .iter()
                .find(|message| &message.id == id)
                .unwrap()
                .created_at,
            if matches!(save, OptimisticSave::AfterFirstRequest) {
                optimistic_timestamp
            } else {
                authoritative_time
            },
            "terminal persistence must be included in the timestamp diagnosis"
        );
    }
    let second = service
        .start_conversation_turn(turn_input(model, 1), notifications)
        .unwrap();
    let second_events = collect_until_done(&mut events).await;
    assert_eq!(
        second_events.last().unwrap()["params"]["status"],
        "completed",
        "{second_events:?}"
    );
    wait_for_worker_release(&service, &second.run_id).await;
    wires.push(requests.recv().await.unwrap());
    provider.await.unwrap();
    wires
}

fn first_message_change(
    before: &LlmRequestFingerprint,
    after: &LlmRequestFingerprint,
) -> Option<usize> {
    assert!(
        !before.messages.is_empty(),
        "a prefix check needs actual messages"
    );
    assert!(
        after.messages.len() > before.messages.len(),
        "this fixture must append, never truncate a request"
    );
    before
        .messages
        .iter()
        .zip(&after.messages)
        .position(|(left, right)| left != right)
}

fn diagnose(save: OptimisticSave, wires: &[Value]) -> Option<usize> {
    let summaries = wires
        .iter()
        .map(fingerprint_llm_request)
        .collect::<Vec<_>>();
    assert_eq!(
        summaries.len(),
        4,
        "three read-workspace requests, then one thanks request"
    );
    let old = wires[2]["messages"].as_array().unwrap();
    assert_eq!(
        old.iter()
            .filter(|message| message["role"] == "tool")
            .count(),
        3
    );
    assert!(
        old.iter().any(|message| message["tool_calls"]
            .as_array()
            .is_some_and(|calls| calls.len() == 2)),
        "the native Provider must exercise a grouped two-file read"
    );
    assert!(old.iter().any(|message| message["reasoning_content"]
        .as_str()
        .is_some_and(|text| !text.is_empty())));
    assert!(
        old.iter().any(|message| message["role"] == "tool"
            && message["content"]
                .as_str()
                .is_some_and(|text| text.contains("A desktop assistant."))),
        "real read_file must reach the model, not just a mocked tool success"
    );
    for (index, summary) in summaries.iter().enumerate() {
        assert_eq!(wires[index]["model"], "deepseek-v4-flash");
        assert_eq!(wires[index]["thinking"], json!({"type": "enabled"}));
        assert_eq!(wires[index]["reasoning_effort"], "high");
        eprintln!(
            "[host-request-prefix] case={save:?} request={index} {}",
            serde_json::to_string(summary).unwrap()
        );
        assert_eq!(
            summary.tools, summaries[0].tools,
            "tools changed at request {index}"
        );
        assert_eq!(summary.system, summaries[0].system);
        assert_eq!(summary.model, summaries[0].model);
    }
    for pair in summaries[..3].windows(2) {
        assert_eq!(
            first_message_change(&pair[0], &pair[1]),
            None,
            "within-Run messages must append"
        );
    }
    let change = first_message_change(&summaries[2], &summaries[3]);
    eprintln!("[host-request-prefix] case={save:?} firstCrossRunChange={change:?}");
    change
}

#[tokio::test]
async fn read_then_thanks_wire_prefix_without_renderer_save() {
    let wires = capture_read_then_thanks(OptimisticSave::Absent).await;
    assert_eq!(diagnose(OptimisticSave::Absent, &wires), None);
}

#[tokio::test]
async fn read_then_thanks_characterizes_late_renderer_timestamp_break() {
    let wires = capture_read_then_thanks(OptimisticSave::AfterFirstRequest).await;
    let change = diagnose(OptimisticSave::AfterFirstRequest, &wires)
        .expect("diagnostic characterization: late optimistic upsert currently changes history");
    let before = wires[2]["messages"].as_array().unwrap();
    let after = wires[3]["messages"].as_array().unwrap();
    let expected = before
        .iter()
        .position(|message| {
            message["content"]
                .as_str()
                .is_some_and(|text| text.ends_with(TASK))
        })
        .unwrap();
    assert_eq!(
        change, expected,
        "first divergence must be the original user timing, before all read-file history"
    );
    assert!(change + 1 < before.len());
    assert_eq!(before[change]["role"], "user");
    let (left, right) = (
        before[change]["content"].as_str().unwrap(),
        after[change]["content"].as_str().unwrap(),
    );
    assert!(left.starts_with("<backend_conversation_timing>"));
    assert_eq!(left.lines().count(), right.lines().count());
    let changed_lines = left
        .lines()
        .zip(right.lines())
        .filter(|(left, right)| left != right)
        .collect::<Vec<_>>();
    assert_eq!(changed_lines.len(), 1);
    assert!(changed_lines[0].0.starts_with("user_message_created_at: "));
    assert!(changed_lines[0].1.starts_with("user_message_created_at: "));
    assert_eq!(
        left.split_once("</backend_conversation_timing>").unwrap().1,
        right
            .split_once("</backend_conversation_timing>")
            .unwrap()
            .1
    );
    assert_eq!(&before[change + 1..], &after[change + 1..before.len()],
        "all old bootstrap, reasoning and complete tool exchanges remain byte-identical after the changed time");
}
