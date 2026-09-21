//! Regression for a prepared request whose safe prefix is compacted before sampling.
use super::*;
use mycopilot_core::{
    AgentConversationWorldStateRequest, WorldStateLifetime, WorldStateRequestBoundary,
    WorldStateSectionEnvelope, WorldStateSectionId,
};
use tokio::net::TcpListener;
use tokio::sync::mpsc::unbounded_channel;

const CONVERSATION: &str = "world-state-compacted-boundary";
const ASSISTANT: &str = "world-state-compacted-assistant";
const RUN: &str = "world-state-compacted-run";
const AFTER_TRACE_MARKER: &str = "PENDING_STATE_AFTER_COMPACTED_TRACE";

fn web_section(marker: &str) -> WorldStateSectionEnvelope {
    let value = json!({"available": true, "boundaryMarker": marker});
    WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::extension("web.search").unwrap(),
        WorldStateLifetime::Conversation,
        value.clone(),
        value,
    )
    .unwrap()
}

#[tokio::test]
async fn prepared_request_survives_exact_trace_compaction_and_real_host_rebuild() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open_with_model_credentials(
            &fixture.path().join("storage.sqlite"),
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION.into(),
            project_id: None,
            model_id: Some("model-1".into()),
            title: "World State compaction".into(),
            messages: vec![
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "world-state-current-user".into(),
                    role: "user".into(),
                    content: "CURRENT_USER_MUST_REMAIN".into(),
                    created_at: 1,
                    status: Some("sent".into()),
                    attachments: vec![],
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: ASSISTANT.into(),
                    role: "assistant".into(),
                    content: "".into(),
                    created_at: 2,
                    status: Some("pending".into()),
                    attachments: vec![],
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new_authorized_for_test(storage.clone());
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": format!("http://{address}/v1/chat/completions"), "apiToken": "test-token", "model": "model-1", "modelCapabilities": {"imageInput":false}, "contextWindowTokens":128000, "messages":[],
    })).unwrap();
    input.context = Some(AgentRunContext {
        conversation_id: Some(CONVERSATION.into()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    });
    freeze_test_pending_provider_configuration(&storage, &mut input);
    let mut trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        RUN,
        CONVERSATION,
        ASSISTANT,
    );
    storage
        .append_in_progress_conversation_turn_trace(&trace, 2, 2)
        .unwrap();
    let host = service.conversation_world_state_host(
        &input,
        RUN,
        CONVERSATION,
        ASSISTANT,
        &AgentCancellationToken::new(),
    );
    let initial_boundary = WorldStateRequestBoundary {
        run_id: RUN.into(),
        assistant_message_id: ASSISTANT.into(),
        request_index: 1,
        after_trace_sequence: None,
    };
    host.prepare_request(AgentConversationWorldStateRequest {
        conversation_id: CONVERSATION.into(),
        boundary: initial_boundary.clone(),
        sections: vec![web_section("INITIAL_STATE")],
    })
    .unwrap();
    // Fixture establishes the state already observed by the response that produced Trace N.
    host.mark_request_observed(&initial_boundary).unwrap();
    trace
        .items
        .push(ConversationTurnTraceItem::AssistantNarration {
            first_tool_call_id: None,
            provider_turn_id: None,
            sequence: 0,
            content: "COVERED_TRACE_NARRATION".into(),
            truncated: false,
        });
    let model_context = vec![ConversationModelContextItem {
        images: Vec::new(),
        sequence: 0,
        ordinal: 0,
        role: "assistant".into(),
        content: "COVERED_TRACE_NARRATION".into(),
        tool_call_id: None,
        tool_calls: vec![],
        is_error: false,
    }];
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &model_context,
            2,
            3,
        )
        .unwrap();
    let pending_boundary = WorldStateRequestBoundary {
        request_index: 2,
        after_trace_sequence: Some(0),
        ..initial_boundary
    };
    let prepared = host
        .prepare_request(AgentConversationWorldStateRequest {
            conversation_id: CONVERSATION.into(),
            boundary: pending_boundary.clone(),
            sections: vec![web_section(AFTER_TRACE_MARKER)],
        })
        .unwrap();
    assert_eq!(prepared.len(), 2);
    assert!(!prepared[1].model_observed);
    let prefix = storage
        .prepare_context_compaction_prefix(
            CONVERSATION,
            &ContextJournalCursor::trace_item(ASSISTANT, 0),
        )
        .unwrap();
    storage
        .commit_context_compaction_prefix(
            &prefix,
            test_compaction_draft(
                &prefix,
                "world-state-trace-summary",
                "SUMMARY_FOR_TRACE_PREFIX",
                100,
                4,
            ),
            ASSISTANT,
        )
        .unwrap();
    let rebased = load_conversation_world_state(&storage, CONVERSATION).unwrap();
    assert_eq!(rebased.len(), 2);
    assert_ne!(rebased[0].record.epoch_id(), prepared[0].record.epoch_id());
    assert_eq!(
        rebased[1].request_boundary.as_ref(),
        Some(&pending_boundary)
    );
    assert!(!rebased[1].model_observed);
    let (notifications, mut events) = unbounded_channel();
    let compaction = service.context_compaction_services(
        RUN,
        CONVERSATION,
        ASSISTANT,
        input,
        RunContextToolProjection::pending(),
        notifications.clone(),
    );
    // The stale plan path calls the same real Host rebuild used after an applied commit.
    let rebuilt = compaction
        .prepare(
            mycopilot_core::AgentContextCompactionPrepareRequest {
                run_id: RUN.into(),
                conversation_id: CONVERSATION.into(),
                assistant_message_id: ASSISTANT.into(),
                expected_previous_summary_id: None,
                covered_through: ContextJournalCursor::trace_item(ASSISTANT, 0),
                source_input_tokens: 100,
                retained_input_tokens: 0,
                uncovered_tail_input_tokens: 100,
                target_replacement_tokens: 20,
            },
            AgentCancellationToken::new(),
        )
        .await;
    match rebuilt {
        Ok(mycopilot_core::AgentContextCompactionPrepareOutcome::Refresh(_)) => {}
        Ok(mycopilot_core::AgentContextCompactionPrepareOutcome::Ready(_)) => {
            panic!("a stale plan must rebuild the committed summary")
        }
        Err(error) => {
            panic!("prepared request after the compacted safe prefix must rebuild: {error}")
        }
    }
    // The fixture run finishes without sending its prepared request. The next real Host request
    // must reconstruct and include that pending diff before it can acknowledge the prefix.
    trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
    storage
        .finalize_chat_message_with_conversation_trace(
            CONVERSATION,
            ASSISTANT,
            "AFTER_SUMMARY_TERMINAL",
            Some("sent"),
            "completed",
            &trace,
            2,
            5,
        )
        .unwrap();
    service.invalidate_conversation_context_state(CONVERSATION);
    let provider = tokio::spawn(async move {
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let request = super::provider_profiles::read_provider_request(&mut stream).await;
        super::provider_profiles::write_provider_stream(
            &mut stream,
            json!({"role":"assistant","content":"FOLLOWUP_COMPLETED"}),
            "stop",
        )
        .await;
        request
    });
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some(CONVERSATION.into()),
                project_id: None,
                model_id: "model-1".into(),
                context_window_indicator_enabled: true,
                content: "FOLLOWUP_USER".into(),
                attachments: vec![],
                folder_references: Vec::new(),
                skills: vec![],
                title: None,
                user_message_id: Some("world-state-followup-user".into()),
                assistant_message_id: Some("world-state-followup-assistant".into()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            notifications,
        )
        .unwrap();
    let terminal = super::provider_profiles::collect_until_done(&mut events).await;
    assert!(
        !terminal
            .iter()
            .any(|event| event["params"]["type"] == "error"),
        "{terminal:?}"
    );
    assert!(terminal
        .iter()
        .any(|event| event["params"]["runId"] == turn.run_id
            && event["params"]["status"] == "completed"));
    let wire = provider.await.unwrap();
    let messages = wire["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            message["content"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| message["content"].to_string())
        })
        .collect::<Vec<_>>();
    let position = |marker: &str| {
        messages
            .iter()
            .position(|text| text.contains(marker))
            .unwrap()
    };
    assert!(position("SUMMARY_FOR_TRACE_PREFIX") < position(AFTER_TRACE_MARKER));
    assert!(position(AFTER_TRACE_MARKER) < position("AFTER_SUMMARY_TERMINAL"));
    assert!(position("AFTER_SUMMARY_TERMINAL") < position("FOLLOWUP_USER"));
    assert!(!messages
        .iter()
        .any(|text| text.contains("COVERED_TRACE_NARRATION")));
    assert!(load_conversation_world_state(&storage, CONVERSATION)
        .unwrap()
        .iter()
        .all(|entry| entry.model_observed));
}
