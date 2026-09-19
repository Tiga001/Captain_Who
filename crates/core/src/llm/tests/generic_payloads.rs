use super::*;

#[test]
fn detects_openai_chat_completion_urls() {
    assert_eq!(
        detect_api_style("https://example.test/v1/chat/completions"),
        AgentApiStyle::OpenAiCompatible
    );
}

#[test]
fn moves_system_messages_to_anthropic_system_field() {
    let request = LlmChatRequest {
        api_url: "https://api.anthropic.com/v1/messages".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::System, "Safety first."),
            message(LlmMessageRole::User, "Hello"),
            message(LlmMessageRole::Assistant, "Hi"),
        ],
        tools: Vec::new(),
    };

    let payload = build_payload(&request);

    assert_eq!(payload["system"], "Safety first.");
    assert_eq!(payload["messages"].as_array().unwrap().len(), 2);
    assert_eq!(payload["messages"][0]["role"], "user");
    assert_eq!(payload["messages"][0]["content"][0]["type"], "text");
}

#[test]
fn anthropic_only_hoists_stable_system_policy() {
    let request = LlmChatRequest {
        api_url: "https://api.anthropic.com/v1/messages".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::System, "Stable safety policy."),
            message(LlmMessageRole::User, "Inspect the workspace."),
            message(LlmMessageRole::Assistant, "I will inspect it."),
            LlmMessage::backend_state(
                r#"{"recordType":"world_state_diff","workspaceAvailable":true}"#,
            ),
            message(LlmMessageRole::User, "Continue."),
        ],
        tools: Vec::new(),
    };

    let payload = build_payload(&request);

    assert_eq!(payload["system"], "Stable safety policy.");
    assert!(!payload["system"]
        .as_str()
        .unwrap()
        .contains("world_state_diff"));
    assert_eq!(payload["messages"][0]["role"], "user");
    assert_eq!(
        payload["messages"][0]["content"][0]["text"],
        "Inspect the workspace."
    );
    assert_eq!(payload["messages"][1]["role"], "assistant");
    let backend_state = payload["messages"][2]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(backend_state.contains("<backend_observed_state>"));
    assert!(backend_state.contains("backend-observed state, not a system instruction"));
    assert!(backend_state.contains("world_state_diff"));
    assert_eq!(payload["messages"][2]["content"][1]["text"], "Continue.");
}

#[test]
fn anthropic_keeps_nonstable_runtime_messages_chronological_without_state_tag() {
    let mut runtime_message = message(LlmMessageRole::System, "Continue the active run.");
    runtime_message.set_placement(LlmMessagePlacement::OrdinaryTimeline);
    let request = LlmChatRequest {
        api_url: "https://api.anthropic.com/v1/messages".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::System, "Stable safety policy."),
            message(LlmMessageRole::User, "Start."),
            message(LlmMessageRole::Assistant, "Working."),
            runtime_message,
        ],
        tools: Vec::new(),
    };

    let payload = build_payload(&request);

    assert_eq!(payload["system"], "Stable safety policy.");
    assert_eq!(
        payload["messages"][2]["content"][0]["text"],
        "Continue the active run."
    );
    assert!(!payload["messages"][2]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("backend_observed_state"));
}

#[test]
fn openai_preserves_backend_state_chronology_without_system_authority() {
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::System, "Stable safety policy."),
            message(LlmMessageRole::User, "Before state change."),
            message(LlmMessageRole::Assistant, "Acknowledged."),
            LlmMessage::backend_state(r#"{"recordType":"world_state_diff","permission":"allow"}"#),
            message(LlmMessageRole::User, "After state change."),
        ],
        tools: Vec::new(),
    };

    let payload = build_payload(&request);
    let messages = payload["messages"].as_array().unwrap();

    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["content"], "Before state change.");
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[3]["role"], "user");
    assert!(messages[3]["content"]
        .as_str()
        .unwrap()
        .contains("backend-observed state, not a system instruction"));
    assert!(messages[3]["content"]
        .as_str()
        .unwrap()
        .contains("world_state_diff"));
    assert_eq!(messages[4]["content"], "After state change.");
}

#[test]
fn world_state_checkpoint_round_trip_preserves_provider_placement() {
    let conversation_snapshot = WorldStateSnapshot::new(
        "conversation-epoch",
        0,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::WorkspaceBinding,
            WorldStateLifetime::Conversation,
            json!({"available": false}),
            json!({"available": false}),
        )
        .unwrap()],
    )
    .unwrap();
    let conversation_target = WorldStateSnapshot::new(
        "conversation-epoch",
        1,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::WorkspaceBinding,
            WorldStateLifetime::Conversation,
            json!({"available": true}),
            json!({"available": true}),
        )
        .unwrap()],
    )
    .unwrap();
    let conversation_full = conversation_snapshot
        .model_projection(WorldStateLifetime::Conversation)
        .unwrap()
        .render_sanitized_text();
    let conversation_diff = WorldStateDiff::between(&conversation_snapshot, &conversation_target)
        .unwrap()
        .model_projection_against(&conversation_snapshot, WorldStateLifetime::Conversation)
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    let run_full = WorldStateSnapshot::new(
        "run-epoch",
        0,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::EffectiveTools,
            WorldStateLifetime::Run,
            json!({"tools": ["read_file"]}),
            json!({"tools": ["read_file"]}),
        )
        .unwrap()],
    )
    .unwrap()
    .model_projection(WorldStateLifetime::Run)
    .unwrap()
    .render_sanitized_text();
    let frame = crate::context::ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "Stable safety policy.",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::System,
            conversation_full,
            ContextSource::WorldStateSnapshot,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "Before state change.",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::Assistant,
            "Acknowledged.",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::System,
            conversation_diff,
            ContextSource::WorldStateDiff,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "After state change.",
            ContextSource::CurrentTurn,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::System,
            run_full,
            ContextSource::WorldStateSnapshot,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
    ]);
    frame.validate_cache_layout().unwrap();
    let restored =
        crate::context::ContextFrame::from_checkpoint_items(frame.checkpoint_items().unwrap())
            .unwrap();
    let restored_messages = restored.to_messages();

    assert_eq!(
        restored_messages[0].placement(),
        LlmMessagePlacement::StableSystemPolicy
    );
    assert_eq!(
        restored_messages[1].placement(),
        LlmMessagePlacement::BackendStateTimeline
    );
    assert_eq!(
        restored_messages[4].placement(),
        LlmMessagePlacement::BackendStateTimeline
    );
    assert_eq!(
        restored_messages[6].placement(),
        LlmMessagePlacement::BackendStateTimeline
    );

    let request = |api_style| LlmChatRequest {
        api_url: "https://example.test".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(api_style),
        provider_protocol: generic_provider_protocol(api_style, "model"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: restored_messages.clone(),
        tools: Vec::new(),
    };

    let openai = build_payload(&request(AgentApiStyle::OpenAiCompatible));
    assert_eq!(openai["messages"][0]["role"], "system");
    assert_eq!(openai["messages"][1]["role"], "user");
    assert!(openai["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"conversation\""));
    assert_eq!(openai["messages"][2]["content"], "Before state change.");
    assert_eq!(openai["messages"][3]["role"], "assistant");
    assert_eq!(openai["messages"][4]["role"], "user");
    assert!(openai["messages"][4]["content"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"conversation\""));
    assert_eq!(openai["messages"][5]["content"], "After state change.");
    assert_eq!(openai["messages"][6]["role"], "user");
    assert!(openai["messages"][6]["content"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"run\""));

    let anthropic = build_payload(&request(AgentApiStyle::AnthropicCompatible));
    assert_eq!(anthropic["system"], "Stable safety policy.");
    assert!(!anthropic["system"]
        .as_str()
        .unwrap()
        .contains("world_state"));
    assert!(anthropic["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"conversation\""));
    assert_eq!(
        anthropic["messages"][0]["content"][1]["text"],
        "Before state change."
    );
    assert_eq!(anthropic["messages"][1]["role"], "assistant");
    assert!(anthropic["messages"][2]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"conversation\""));
    assert_eq!(
        anthropic["messages"][2]["content"][1]["text"],
        "After state change."
    );
    assert!(anthropic["messages"][2]["content"][2]["text"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"run\""));
}

#[test]
fn message_placement_distinguishes_stable_policy_from_timeline_state() {
    let stable = message(LlmMessageRole::System, "policy");
    let backend_state = LlmMessage::backend_state("state");
    let ordinary = message(LlmMessageRole::User, "request");

    assert_eq!(stable.placement(), LlmMessagePlacement::StableSystemPolicy);
    assert_eq!(
        backend_state.placement(),
        LlmMessagePlacement::BackendStateTimeline
    );
    assert_eq!(ordinary.placement(), LlmMessagePlacement::OrdinaryTimeline);
}

#[test]
fn omits_temperature_for_claude_models() {
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(
            AgentApiStyle::OpenAiCompatible,
            "claude-opus-4-7",
        ),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: true,
        messages: vec![message(LlmMessageRole::User, "Hello")],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);

    assert!(payload.get("temperature").is_none());
    assert_eq!(payload["stream_options"]["include_usage"], true);
}

#[test]
fn extracts_model_error_payloads() {
    let value = json!({
        "error": {
            "code": "BIZ_ERROR",
            "message": "upstream status 400"
        }
    });

    assert_eq!(
        extract_api_error(&value).as_deref(),
        Some("upstream status 400")
    );
}

#[test]
fn builds_openai_native_tool_payload_and_tool_result_messages() {
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::User, "Read src/lib.rs"),
            LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/lib.rs" }),
                }],
            ),
            LlmMessage::tool_result("call-1", "{\"ok\":true}", false),
        ],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);

    assert_eq!(payload["tool_choice"], "auto");
    assert_eq!(payload["tools"][0]["type"], "function");
    assert_eq!(payload["tools"][0]["function"]["name"], "read_file");
    assert_eq!(payload["messages"][1]["tool_calls"][0]["id"], "call-1");
    assert_eq!(payload["messages"][2]["role"], "tool");
    assert_eq!(payload["messages"][2]["tool_call_id"], "call-1");
}

#[test]
fn generic_provider_payloads_expose_only_the_current_file_change_tool() {
    let registry = crate::tools::ToolRegistry::defaults_with_search(None);
    assert!(registry.definition_for("write_file").is_none());
    let definitions = ["apply_patch"]
        .into_iter()
        .map(|name| {
            registry
                .definition_for(name)
                .unwrap_or_else(|| panic!("missing stable tool definition for {name}"))
        })
        .collect::<Vec<_>>();

    let apply_patch = &definitions[0];
    assert_eq!(apply_patch.safety, AgentToolSafety::RequiresApproval);
    assert!(!apply_patch.requires_workspace);
    assert!(apply_patch.requires_approval);
    assert_eq!(
        apply_patch.approval_mode,
        crate::protocol::AgentToolApprovalMode::Dynamic
    );
    assert!(apply_patch.description.contains("read_file"));
    assert_eq!(apply_patch.input_schema["required"], json!(["request"]));
    assert_eq!(apply_patch.input_schema["additionalProperties"], false);
    let request_branches = apply_patch.input_schema["properties"]["request"]["oneOf"]
        .as_array()
        .unwrap();
    assert_eq!(request_branches.len(), 11);
    assert_eq!(
        request_branches[2]["properties"]["edits"]["items"]["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    let apply_properties = apply_patch.input_schema["properties"].as_object().unwrap();
    assert_eq!(apply_properties.len(), 1);
    assert!(apply_properties.contains_key("request"));
    assert!(!apply_properties.contains_key("patch"));
    assert!(!apply_properties.contains_key("expectedRevision"));
    assert_eq!(
        request_branches[0]["properties"]["content"]["maxLength"],
        32 * 1024
    );
    assert_eq!(
        request_branches[6]["properties"]["content"]["maxLength"],
        1024 * 1024
    );
    let request = |api_style, tools: Vec<AgentToolDefinition>| LlmChatRequest {
        api_url: "https://example.test".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(api_style),
        provider_protocol: generic_provider_protocol(api_style, "model"),
        max_tokens: Some(1_024),
        temperature: 0.2,
        stream: false,
        messages: vec![message(LlmMessageRole::User, "Update a file")],
        tools,
    };

    let openai = build_payload(&request(
        AgentApiStyle::OpenAiCompatible,
        definitions.clone(),
    ));
    let openai_tools = openai["tools"].as_array().unwrap();
    assert_eq!(openai_tools.len(), 1);
    for (projected, definition) in openai_tools.iter().zip(&definitions) {
        assert_eq!(projected["type"], "function");
        assert_eq!(projected["function"]["name"], definition.name);
        assert_eq!(projected["function"]["description"], definition.description);
        assert_eq!(projected["function"]["parameters"], definition.input_schema);
        assert_eq!(projected.as_object().unwrap().len(), 2);
        assert_eq!(projected["function"].as_object().unwrap().len(), 3);
    }

    let anthropic = build_payload(&request(
        AgentApiStyle::AnthropicCompatible,
        definitions.clone(),
    ));
    let anthropic_tools = anthropic["tools"].as_array().unwrap();
    assert_eq!(anthropic_tools.len(), 1);
    for (projected, definition) in anthropic_tools.iter().zip(&definitions) {
        assert_eq!(projected["name"], definition.name);
        assert_eq!(projected["description"], definition.description);
        assert_eq!(projected["input_schema"], definition.input_schema);
        assert_eq!(projected.as_object().unwrap().len(), 3);
    }
}

#[test]
fn generic_provider_payloads_preserve_strict_direct_apply_call_and_result_order() {
    let args = json!({
        "request": {
            "action": "apply",
            "operation": "update",
            "filePath": "README.md",
            "observationId": "fobs_current_read",
            "edits": [{
                "kind": "replace",
                "oldText": "old heading",
                "newText": "new heading"
            }],
            "summary": "Update the heading"
        }
    });

    for api_style in [
        AgentApiStyle::OpenAiCompatible,
        AgentApiStyle::AnthropicCompatible,
    ] {
        let model = if api_style == AgentApiStyle::OpenAiCompatible {
            "gpt"
        } else {
            "claude"
        };
        let request = LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: generic_provider_profile(api_style),
            provider_protocol: generic_provider_protocol(api_style, model),
            max_tokens: Some(1_024),
            temperature: 0.2,
            stream: false,
            messages: vec![
                message(LlmMessageRole::User, "Update README.md"),
                LlmMessage::assistant(
                    "",
                    vec![LlmToolCall {
                        id: "call-direct-apply".to_string(),
                        name: "apply_patch".to_string(),
                        args: args.clone(),
                    }],
                ),
                LlmMessage::tool_result("call-direct-apply", r#"{"status":"applied"}"#, false),
            ],
            tools: Vec::new(),
        };

        let payload = build_payload(&request);
        if api_style == AgentApiStyle::OpenAiCompatible {
            assert_eq!(payload["messages"][1]["role"], "assistant");
            assert_eq!(
                payload["messages"][1]["tool_calls"][0]["function"]["name"],
                "apply_patch"
            );
            let wire_args: Value = serde_json::from_str(
                payload["messages"][1]["tool_calls"][0]["function"]["arguments"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(wire_args, args);
            assert_eq!(payload["messages"][2]["role"], "tool");
            assert_eq!(payload["messages"][2]["tool_call_id"], "call-direct-apply");
        } else {
            assert_eq!(payload["messages"][1]["role"], "assistant");
            assert_eq!(payload["messages"][1]["content"][0]["type"], "tool_use");
            assert_eq!(payload["messages"][1]["content"][0]["name"], "apply_patch");
            assert_eq!(payload["messages"][1]["content"][0]["input"], args);
            assert_eq!(payload["messages"][2]["role"], "user");
            assert_eq!(
                payload["messages"][2]["content"][0]["tool_use_id"],
                "call-direct-apply"
            );
        }
    }
}

#[test]
fn generic_provider_payloads_preserve_staged_apply_patch_history_and_exact_arguments() {
    let calls = [
        (
            "call-staged-begin",
            json!({
                "request": {
                    "action": "begin",
                    "operation": "create",
                    "filePath": "report.md"
                }
            }),
            r#"{"transactionId":"file-change-staged-v1:report","draftRevision":0,"nextIndex":0}"#,
        ),
        (
            "call-staged-append",
            json!({
                "request": {
                    "action": "append",
                    "transactionId": "file-change-staged-v1:report",
                    "index": 0,
                    "expectedDraftRevision": 0,
                    "content": "# Report\n"
                }
            }),
            r#"{"transactionId":"file-change-staged-v1:report","draftRevision":1,"nextIndex":1}"#,
        ),
        (
            "call-staged-commit",
            json!({
                "request": {
                    "action": "commit",
                    "transactionId": "file-change-staged-v1:report",
                    "expectedDraftRevision": 1,
                    "summary": "Create report"
                }
            }),
            r#"{"status":"applied","filePath":"report.md"}"#,
        ),
    ];

    for api_style in [
        AgentApiStyle::OpenAiCompatible,
        AgentApiStyle::AnthropicCompatible,
    ] {
        let model = if api_style == AgentApiStyle::OpenAiCompatible {
            "gpt"
        } else {
            "claude"
        };
        let mut messages = vec![message(LlmMessageRole::User, "Create report.md")];
        for (id, args, result) in &calls {
            messages.push(LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: (*id).to_string(),
                    name: "apply_patch".to_string(),
                    args: args.clone(),
                }],
            ));
            messages.push(LlmMessage::tool_result(*id, *result, false));
        }
        let payload = build_payload(&LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: generic_provider_profile(api_style),
            provider_protocol: generic_provider_protocol(api_style, model),
            max_tokens: Some(1_024),
            temperature: 0.2,
            stream: false,
            messages,
            tools: Vec::new(),
        });

        let wire_messages = payload["messages"].as_array().unwrap();
        assert_eq!(wire_messages.len(), 7);
        for (index, (id, args, _)) in calls.iter().enumerate() {
            let assistant = &wire_messages[1 + index * 2];
            let result = &wire_messages[2 + index * 2];
            if api_style == AgentApiStyle::OpenAiCompatible {
                let call = &assistant["tool_calls"][0];
                assert_eq!(call["id"], *id);
                assert_eq!(call["function"]["name"], "apply_patch");
                assert_eq!(
                    serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap())
                        .unwrap(),
                    *args
                );
                assert_eq!(result["role"], "tool");
                assert_eq!(result["tool_call_id"], *id);
            } else {
                let call = &assistant["content"][0];
                assert_eq!(call["type"], "tool_use");
                assert_eq!(call["id"], *id);
                assert_eq!(call["name"], "apply_patch");
                assert_eq!(call["input"], *args);
                assert_eq!(result["role"], "user");
                assert_eq!(result["content"][0]["tool_use_id"], *id);
            }
        }
    }
}

#[test]
fn builds_openai_mcp_namespace_tool_payload_without_internal_catalog_fields() {
    let definition = mcp_tool_definition();
    let expected_schema = definition.input_schema.clone();
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![message(LlmMessageRole::User, "Add the numbers")],
        tools: vec![definition],
    };

    let payload = build_payload(&request);
    let tool = &payload["tools"][0];

    assert_eq!(
        tool,
        &json!({
            "type": "function",
            "function": {
                "name": "mcp__fixture__add_numbers",
                "description": "MCP server: \"Fixture MCP\"\nMCP tool: \"add_numbers\"\nDescription: Add two fixture numbers.",
                "parameters": expected_schema
            }
        })
    );
    assert_eq!(tool["function"]["parameters"]["type"], "object");
    assert_eq!(tool["function"].as_object().unwrap().len(), 3);

    let encoded = serde_json::to_string(tool).unwrap();
    for internal_field in [
        "provenance",
        "outputSchema",
        "output_schema",
        "_meta",
        "serverId",
        "rawToolName",
        "configDigest",
        "catalogGeneration",
    ] {
        assert!(
            !encoded.contains(&format!("\"{internal_field}\"")),
            "OpenAI tool payload leaked internal MCP field {internal_field}: {encoded}"
        );
    }
}

#[test]
fn builds_anthropic_mcp_namespace_tool_payload_without_internal_catalog_fields() {
    let definition = mcp_tool_definition();
    let expected_schema = definition.input_schema.clone();
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/messages".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![message(LlmMessageRole::User, "Add the numbers")],
        tools: vec![definition],
    };

    let payload = build_payload(&request);
    let tool = &payload["tools"][0];

    assert_eq!(
        tool,
        &json!({
            "name": "mcp__fixture__add_numbers",
            "description": "MCP server: \"Fixture MCP\"\nMCP tool: \"add_numbers\"\nDescription: Add two fixture numbers.",
            "input_schema": expected_schema
        })
    );
    assert_eq!(tool["input_schema"]["type"], "object");
    assert_eq!(tool.as_object().unwrap().len(), 3);

    let encoded = serde_json::to_string(tool).unwrap();
    for internal_field in [
        "provenance",
        "outputSchema",
        "output_schema",
        "_meta",
        "serverId",
        "rawToolName",
        "configDigest",
        "catalogGeneration",
    ] {
        assert!(
            !encoded.contains(&format!("\"{internal_field}\"")),
            "Anthropic tool payload leaked internal MCP field {internal_field}: {encoded}"
        );
    }
}

#[test]
fn assembled_context_preserves_order_across_provider_payloads() {
    let mut timestamped_history = chat_message("user", "Earlier question");
    timestamped_history.created_at = Some(0);
    let mut timestamped_answer =
        traced_narration_chat_message("assistant-timestamped", "Earlier answer");
    timestamped_answer.created_at = Some(1_000);
    let mut timestamped_current = chat_message("user", "Continue the edit");
    timestamped_current.created_at = Some(2_000);
    let mut context = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "System rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![timestamped_history, timestamped_answer, timestamped_current],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();
    let group = ContextGroup::tool_exchange("tool-context-1");
    context.push(ContextItem::assistant(
        "",
        vec![LlmToolCall {
            id: "call-context-1".to_string(),
            name: "read_file".to_string(),
            args: json!({ "path": "src/lib.rs" }),
        }],
        ContextMetadata::new(
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_group(group.clone()),
    ));
    context.push(ContextItem::tool_result(
        "call-context-1",
        "file contents",
        false,
        ContextMetadata::new(
            ContextSource::ToolResult,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_group(group),
    ));

    let request = |api_style| LlmChatRequest {
        api_url: "https://example.test".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(api_style),
        provider_protocol: generic_provider_protocol(api_style, "model"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: context.to_messages(),
        tools: vec![tool_definition()],
    };

    let openai = build_payload(&request(AgentApiStyle::OpenAiCompatible));
    assert_eq!(openai["messages"][0]["role"], "system");
    let expected_timestamped_history = format!(
            "<backend_conversation_timing>\nuser_message_created_at: {}\n</backend_conversation_timing>\nEarlier question",
            format_message_created_at(0).unwrap()
        );
    let expected_timestamped_current = format!(
            "<backend_conversation_timing>\nprevious_assistant_message_created_at: {}\nuser_message_created_at: {}\n</backend_conversation_timing>\nContinue the edit",
            format_message_created_at(1_000).unwrap(),
            format_message_created_at(2_000).unwrap()
        );
    assert_eq!(
        openai["messages"][1]["content"],
        expected_timestamped_history
    );
    assert_eq!(openai["messages"][2]["content"], "Earlier answer");
    assert!(openai["messages"][3]["content"]
        .as_str()
        .unwrap()
        .contains("historical_agent_activity_terminal"));
    assert_eq!(
        openai["messages"][4]["content"],
        expected_timestamped_current
    );
    assert_eq!(
        openai["messages"][5]["tool_calls"][0]["id"],
        "call-context-1"
    );
    assert_eq!(openai["messages"][6]["role"], "tool");

    let anthropic = build_payload(&request(AgentApiStyle::AnthropicCompatible));
    assert_eq!(anthropic["system"], "System rules");
    assert_eq!(
        anthropic["messages"][0]["content"][0]["text"],
        expected_timestamped_history
    );
    assert_eq!(
        anthropic["messages"][1]["content"][0]["text"],
        "Earlier answer"
    );
    assert!(anthropic["messages"][1]["content"][1]["text"]
        .as_str()
        .unwrap()
        .contains("historical_agent_activity_terminal"));
    assert_eq!(
        anthropic["messages"][2]["content"][0]["text"],
        expected_timestamped_current
    );
    assert_eq!(
        anthropic["messages"][3]["content"][0]["id"],
        "call-context-1"
    );
    assert_eq!(
        anthropic["messages"][4]["content"][0]["tool_use_id"],
        "call-context-1"
    );
}

#[test]
fn conversation_trace_builds_legal_ordered_tool_history_for_both_providers() {
    let context = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "System rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![
            chat_message("user", "Inspect the file"),
            traced_chat_message("The file is valid."),
            chat_message("user", "What did you inspect?"),
        ],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();
    context.validate_complete_tool_protocol().unwrap();

    let request = |api_style| LlmChatRequest {
        api_url: "https://example.test".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(api_style),
        provider_protocol: generic_provider_protocol(api_style, "model"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: context.to_messages(),
        tools: vec![tool_definition()],
    };

    let openai = build_payload(&request(AgentApiStyle::OpenAiCompatible));
    assert_eq!(openai["messages"][1]["content"], "Inspect the file");
    assert_eq!(
        openai["messages"][2]["content"],
        "I will inspect src/lib.rs."
    );
    let openai_call_id = openai["messages"][3]["tool_calls"][0]["id"]
        .as_str()
        .unwrap();
    assert_eq!(openai_call_id, historical_trace_call_id());
    assert_eq!(openai_call_id.len(), 47);
    assert!(openai_call_id.starts_with("tc1_"));
    assert_eq!(openai["messages"][3]["content"], Value::Null);
    assert_eq!(openai["messages"][4]["role"], "tool");
    assert_eq!(openai["messages"][4]["tool_call_id"], openai_call_id);
    assert!(!openai["messages"][4]["content"]
        .as_str()
        .unwrap()
        .contains("\"ok\""));
    assert_eq!(openai["messages"][5]["content"], "The file is valid.");
    assert!(openai["messages"][6]["content"]
        .as_str()
        .unwrap()
        .contains("historical_agent_activity_terminal"));
    assert_eq!(openai["messages"][7]["content"], "What did you inspect?");

    let anthropic = build_payload(&request(AgentApiStyle::AnthropicCompatible));
    assert_eq!(anthropic["system"], "System rules");
    assert_eq!(anthropic["messages"][0]["role"], "user");
    assert_eq!(
        anthropic["messages"][1]["content"][0]["text"],
        "I will inspect src/lib.rs."
    );
    assert_eq!(anthropic["messages"][1]["content"][1]["type"], "tool_use");
    let anthropic_call_id = anthropic["messages"][1]["content"][1]["id"]
        .as_str()
        .unwrap();
    assert_eq!(anthropic_call_id, historical_trace_call_id());
    assert_eq!(anthropic_call_id, openai_call_id);
    assert_eq!(anthropic["messages"][2]["role"], "user");
    assert_eq!(
        anthropic["messages"][2]["content"][0]["type"],
        "tool_result"
    );
    assert_eq!(
        anthropic["messages"][2]["content"][0]["tool_use_id"],
        anthropic_call_id
    );
    assert_eq!(anthropic["messages"][2]["content"][0]["is_error"], false);
    assert_eq!(
        anthropic["messages"][3]["content"][0]["text"],
        "The file is valid."
    );
    assert!(anthropic["messages"][3]["content"][1]["text"]
        .as_str()
        .unwrap()
        .contains("historical_agent_activity_terminal"));
    assert_eq!(
        anthropic["messages"][4]["content"][0]["text"],
        "What did you inspect?"
    );
}

#[test]
fn builds_openai_image_messages_with_image_url_parts() {
    let mut image_message = message(LlmMessageRole::User, "Inspect this image.");
    image_message.images_mut().unwrap().push(LlmImage {
        mime_type: "image/png".to_string(),
        data_base64: "YWJj".to_string(),
    });
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![image_message],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);

    assert_eq!(payload["messages"][0]["content"][0]["type"], "text");
    assert_eq!(payload["messages"][0]["content"][1]["type"], "image_url");
    assert!(payload["messages"][0]["content"][1].get("image").is_none());
}

#[test]
fn builds_anthropic_native_tool_payload_and_tool_result_messages() {
    let request = LlmChatRequest {
        api_url: "https://api.anthropic.com/v1/messages".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::User, "Read src/lib.rs"),
            LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: "toolu-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/lib.rs" }),
                }],
            ),
            LlmMessage::tool_result("toolu-1", "{\"ok\":true}", false),
        ],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);

    assert_eq!(payload["tools"][0]["name"], "read_file");
    assert_eq!(payload["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(payload["messages"][2]["role"], "user");
    assert_eq!(payload["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(
        payload["messages"][2]["content"][0]["tool_use_id"],
        "toolu-1"
    );
}

#[test]
fn extracts_native_tool_calls() {
    let openai = json!({
        "choices": [{
            "message": {
                "tool_calls": [{
                    "id": " call-1 ",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"src/lib.rs\"}"
                    }
                }]
            }
        }]
    });
    let anthropic = json!({
        "content": [{
            "type": "tool_use",
            "id": " toolu-1 ",
            "name": "search_files",
            "input": { "query": "main" }
        }]
    });

    let openai_calls = extract_tool_calls(&openai, AgentApiStyle::OpenAiCompatible).unwrap();
    let anthropic_calls =
        extract_tool_calls(&anthropic, AgentApiStyle::AnthropicCompatible).unwrap();

    assert_eq!(openai_calls[0].id, " call-1 ");
    assert_eq!(openai_calls[0].name, "read_file");
    assert_eq!(openai_calls[0].args["path"], "src/lib.rs");
    assert_eq!(anthropic_calls[0].id, " toolu-1 ");
    assert_eq!(anthropic_calls[0].name, "search_files");
    assert_eq!(anthropic_calls[0].args["query"], "main");
}

#[test]
fn provider_tool_call_ids_preserve_the_byte_boundary_and_short_duplicates() {
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt");
    let boundary_id = "é".repeat(MAX_PROVIDER_TOOL_CALL_ID_BYTES / "é".len());
    assert_eq!(boundary_id.len(), MAX_PROVIDER_TOOL_CALL_ID_BYTES);
    let duplicate_id = " provider-opaque-id ";
    let calls = vec![
        LlmToolCall {
            id: boundary_id.clone(),
            name: "read_file".to_string(),
            args: json!({}),
        },
        LlmToolCall {
            id: duplicate_id.to_string(),
            name: "read_file".to_string(),
            args: json!({}),
        },
        LlmToolCall {
            id: duplicate_id.to_string(),
            name: "read_file".to_string(),
            args: json!({}),
        },
    ];

    let turn = LlmAssistantTurn::from_provider(protocol.clone(), "", calls).unwrap();
    assert_eq!(turn.provider_tool_calls()[0].id, boundary_id);
    assert_eq!(turn.provider_tool_calls()[1].id, duplicate_id);
    assert_eq!(turn.provider_tool_calls()[2].id, duplicate_id);

    let oversized = LlmAssistantTurn::from_provider(
        protocol,
        "",
        vec![LlmToolCall {
            id: "x".repeat(MAX_PROVIDER_TOOL_CALL_ID_BYTES + 1),
            name: "read_file".to_string(),
            args: json!({}),
        }],
    )
    .unwrap_err();
    assert_eq!(
        oversized.code(),
        Some("agent.invalid_provider_tool_call_id")
    );

    let empty = LlmAssistantTurn::from_provider(
        generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        "",
        vec![LlmToolCall {
            id: String::new(),
            name: "read_file".to_string(),
            args: json!({}),
        }],
    )
    .unwrap_err();
    assert_eq!(empty.code(), Some("agent.invalid_provider_tool_call_id"));

    let forged_provider_call = LlmToolCall {
        id: "x".repeat(MAX_PROVIDER_TOOL_CALL_ID_BYTES + 1),
        name: "read_file".to_string(),
        args: json!({}),
    };
    let forged_binding = LlmRuntimeToolCallBinding::new(
        0,
        &forged_provider_call,
        LlmToolCall {
            id: model_response_tool_call_id("forged-checkpoint", 0, 0, "provider"),
            name: forged_provider_call.name.clone(),
            args: forged_provider_call.args.clone(),
        },
    );
    let forged = LlmAssistantTurn::from_split_projection("", vec![forged_provider_call])
        .with_runtime_tool_bindings(vec![forged_binding])
        .unwrap_err();
    assert_eq!(forged.code(), Some("agent.invalid_provider_tool_call_id"));
}

#[test]
fn oversized_provider_tool_call_ids_fail_closed_nonstream_and_stream() {
    let oversized_id = "x".repeat(MAX_PROVIDER_TOOL_CALL_ID_BYTES + 1);
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt");
    let nonstream = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": oversized_id,
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap_err();
    assert_eq!(
        nonstream.code(),
        Some("agent.invalid_provider_tool_call_id")
    );

    let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": oversized_id,
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
        ),
        &mut accumulator,
        &mut |_| {},
    )
    .unwrap();
    let stream = accumulator.finish().unwrap_err();
    assert_eq!(stream.code(), Some("agent.invalid_provider_tool_call_id"));
}

#[test]
fn provider_checkpoint_identity_debug_is_redacted() {
    const CANARY: &str = "RAW_PROVIDER_CALL_ID_DEBUG_CANARY";
    let mapping = AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: CANARY.to_string(),
        runtime_call_id: "runtime-id".to_string(),
    };
    let identity = AgentAssistantTurnCheckpointIdentity {
        assistant_turn_id: "turn-id".to_string(),
        assistant_turn_digest: "digest".to_string(),
        tool_call_identities: vec![mapping.clone()],
    };

    assert!(!format!("{mapping:?}").contains(CANARY));
    assert!(!format!("{identity:?}").contains(CANARY));
}
