use super::*;

#[tokio::test]
async fn provider_payload_orders_current_input_before_run_bootstrap_and_preserves_the_live_tool_exchange(
) {
    async fn run_case(style: crate::AgentApiStyle) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                requests.push(read_runtime_test_json_request(&mut stream).await);
                let response = match (style, index) {
                    (crate::AgentApiStyle::OpenAiCompatible, 0) => json!({
                        "choices": [{"message": {
                            "role": "assistant", "content": "LAYOUT_LIVE_NARRATION",
                            "tool_calls": [{"id": "layout-todo", "type": "function", "function": {
                                "name": "todo_update", "arguments": json!({"items": [{
                                    "title": "LAYOUT_CURRENT_TODO", "status": "in_progress"
                                }]}).to_string()
                            }}]
                        }, "finish_reason": "tool_calls"}]
                    }),
                    (crate::AgentApiStyle::OpenAiCompatible, _) => json!({
                        "choices": [{"message": {"role": "assistant", "content": "done"},
                            "finish_reason": "stop"}]
                    }),
                    (crate::AgentApiStyle::AnthropicCompatible, 0) => json!({
                        "content": [
                            {"type": "text", "text": "LAYOUT_LIVE_NARRATION"},
                            {"type": "tool_use", "id": "layout-todo", "name": "todo_update", "input": {
                                "items": [{"title": "LAYOUT_CURRENT_TODO", "status": "in_progress"}]
                            }}
                        ], "stop_reason": "tool_use"
                    }),
                    (crate::AgentApiStyle::AnthropicCompatible, _) => json!({
                        "content": [{"type": "text", "text": "done"}], "stop_reason": "end_turn"
                    }),
                };
                write_runtime_test_json_response(&mut stream, response).await;
            }
            requests
        });

        let mut input = conversation_context_input(vec![
            message("user", "LAYOUT_OLD_USER"),
            current_assistant_history_message("LAYOUT_OLD_ASSISTANT"),
            message("user", "当前任务补充一"),
            message("user", "当前任务补充二"),
        ]);
        input.messages[2].message_id = Some("layout-input-one".to_string());
        input.messages[3].message_id = Some("layout-input-two".to_string());
        input.api_url = format!("http://{address}/v1/messages");
        input.api_token = "test-token".to_string();
        input.api_style = Some(style);
        freeze_runtime_test_generic_provider(&mut input, "request-layout-bootstrap");
        input.stream = Some(false);
        let activation = activated_skill("LAYOUT_PREACTIVATED_SKILL");
        let active_skill = &activation.skills[0];
        let mut discovery = discoverable_skill("LAYOUT_SKILL_CATALOG");
        discovery.skills[0].id = active_skill.id.clone();
        discovery.skills[0].revision = active_skill.revision.clone();
        discovery.skills[0].name = active_skill.name.clone();
        discovery.skills[0].source_kind = "workspace".to_string();
        discovery.skills[0].activation_ref = crate::skills::derive_skill_activation_ref(
            &discovery.catalog_revision,
            &active_skill.id,
            &active_skill.revision,
        );
        input.skill_discovery = Some(discovery);
        input.skill_activation = Some(activation);
        input.search_config = Some(AgentSearchConfig {
            mode: AgentSearchMode::Auto,
            tavily_api_key: Some("unused-layout-key".to_string()),
        });
        let attachment_directory = tempfile::tempdir().unwrap();
        let (attachment, library) = managed_runtime_attachment(
            attachment_directory.path(),
            "layout-attachment",
            "说明.txt",
            "text/plain",
            AgentInputAttachmentKind::File,
            "中文附件内容".as_bytes(),
        );
        input.attachments = vec![attachment];
        set_runtime_attachment_library(&mut input, library);
        let conversation_snapshot = |sequence, marker| {
            let state = json!({"marker": marker});
            WorldStateSnapshot::new(
                "layout-conversation",
                sequence,
                vec![WorldStateSectionEnvelope::model_visible(
                    WorldStateSectionId::extension("layout.fixture").unwrap(),
                    WorldStateLifetime::Conversation,
                    state.clone(),
                    state,
                )
                .unwrap()],
            )
            .unwrap()
        };
        let initial_conversation = conversation_snapshot(0, "LAYOUT_CONVERSATION_STATE");
        let input_difference = crate::WorldStateDiff::between(
            &initial_conversation,
            &conversation_snapshot(1, "LAYOUT_INPUT_STATE_DIFF"),
        )
        .unwrap();
        input.world_state_records = vec![
            AnchoredWorldStateRecord::new(WorldStateRecord::Full(initial_conversation), None)
                .unwrap(),
            AnchoredWorldStateRecord::new(
                WorldStateRecord::Diff(input_difference),
                Some("layout-input-two".to_string()),
            )
            .unwrap(),
        ];

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            AgentRuntime::default().send_chat_with_events_and_cancellation(
                input,
                Some("request-layout-bootstrap".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(
                    AgentRuntimeHostServices::new()
                        .with_skill_resources(activated_skill_authority()),
                ),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(output.content, "done");
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["tools"], requests[1]["tools"]);
        match style {
            crate::AgentApiStyle::OpenAiCompatible => {
                assert_eq!(requests[0]["messages"][0], requests[1]["messages"][0]);
            }
            crate::AgentApiStyle::AnthropicCompatible => {
                assert_eq!(requests[0]["system"], requests[1]["system"]);
            }
        }
        for request in &requests {
            let context = request["messages"].to_string();
            let markers = [
                "LAYOUT_SKILL_CATALOG",
                "## 联网搜索",
                "LAYOUT_CONVERSATION_STATE",
                "LAYOUT_OLD_USER",
                "LAYOUT_OLD_ASSISTANT",
                "当前任务补充一",
                "LAYOUT_INPUT_STATE_DIFF",
                "当前任务补充二",
                "中文附件内容",
                "LAYOUT_PREACTIVATED_SKILL",
                "skills.activation",
            ];
            let positions = markers.map(|marker| {
                assert_eq!(context.matches(marker).count(), 1, "{marker}: {context}");
                context.find(marker).unwrap()
            });
            assert!(
                positions.windows(2).all(|pair| pair[0] < pair[1]),
                "{style:?}: {context}"
            );
            assert!(!request.to_string().contains("unused-layout-key"));
            for private_inventory_field in ["tools.effective", "stableTools", "dynamicTools"] {
                assert!(!context.contains(private_inventory_field), "{context}");
            }
        }
        let second = requests[1]["messages"].to_string();
        assert!(
            second.find("skills.activation").unwrap()
                < second.find("LAYOUT_LIVE_NARRATION").unwrap()
        );
        let call_id = output
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolCall { call, .. } if call.tool == "todo_update" => {
                    Some(call.id.as_str())
                }
                _ => None,
            })
            .unwrap();
        assert_runtime_owned_tool_call_id(call_id);
        let messages = requests[1]["messages"].as_array().unwrap();
        let (call_index, result_index) = match style {
            crate::AgentApiStyle::OpenAiCompatible => (
                messages
                    .iter()
                    .position(|message| message["tool_calls"][0]["id"] == call_id)
                    .unwrap(),
                messages
                    .iter()
                    .position(|message| {
                        message["role"] == "tool" && message["tool_call_id"] == call_id
                    })
                    .unwrap(),
            ),
            crate::AgentApiStyle::AnthropicCompatible => {
                let block_index = |kind: &str, id_key: &str| {
                    messages
                        .iter()
                        .position(|message| {
                            message["content"].as_array().is_some_and(|blocks| {
                                blocks
                                    .iter()
                                    .any(|block| block["type"] == kind && block[id_key] == call_id)
                            })
                        })
                        .unwrap()
                };
                (
                    block_index("tool_use", "id"),
                    block_index("tool_result", "tool_use_id"),
                )
            }
        };
        assert_eq!(result_index, call_index + 1, "{style:?}: {second}");
        assert!(!second.contains("layout-todo"));
    }

    run_case(crate::AgentApiStyle::OpenAiCompatible).await;
    run_case(crate::AgentApiStyle::AnthropicCompatible).await;
}
