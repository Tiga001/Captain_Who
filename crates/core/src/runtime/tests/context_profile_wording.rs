use super::*;
use crate::protocol::AgentContextProfile;

fn wording_input(profile: AgentContextProfile) -> AgentChatInput {
    let mut input = conversation_context_input(vec![message("user", "你好")]);
    input.api_token = "local-wording-fixture-only".into();
    input.stream = Some(false);
    input.prompt_preferences =
        Some(serde_json::from_value(json!({"contextProfile": profile})).unwrap());
    freeze_runtime_test_generic_provider(&mut input, "wording-measurement");
    input
}

async fn capture_wording_payload(mut request: crate::llm::LlmChatRequest) -> Value {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    request.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let capture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let payload = read_runtime_test_json_request(&mut stream).await;
        let response = json!({"choices":[{"message":{"role":"assistant","content":"你好"},"finish_reason":"stop"}]});
        let body = serde_json::to_vec(&response).unwrap();
        let headers = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
        payload
    });
    crate::llm::complete_chat(request, AgentCancellationToken::new())
        .await
        .unwrap();
    capture.await.unwrap()
}

#[tokio::test]
async fn minimal_native_request_keeps_system_and_tool_guidance_as_one_contract() {
    let input = wording_input(AgentContextProfile::Minimal);
    let capabilities =
        prepare_runtime_capabilities(&input, "wording-contract", &[], true, None).unwrap();
    let prepared = build_llm_request(
        input,
        capabilities.initial_tool_set.stable_definitions(),
        None,
        None,
        None,
    )
    .unwrap();
    let payload = capture_wording_payload(prepared.template.request(prepared.context, &[])).await;
    let system = payload["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "system")
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let native_tool = |name: &str| {
        payload["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == name)
            .unwrap_or_else(|| panic!("missing native tool {name}"))["function"]
            .clone()
    };
    // Both layers are assembled for the same native request. Moving guidance between layers
    // must not accidentally remove it from both or leave it only in an unused full definition.
    for invariant in [
        "permissions.effective",
        "require_approval",
        "MCP",
        "Backend file transaction state",
        "children have independent Runs",
        "frozen catalog ref as skills_activate.skillRef",
        "full instructions/new tools apply next model request only",
        "Only explicit activated-Skill copy-first",
        "Copy returned readPath into a matching reader",
        "Use supplied native schemas",
        "Respond naturally in the user's language",
        "not command/write permission",
    ] {
        assert!(
            system.contains(invariant),
            "missing system contract: {invariant}"
        );
    }
    let patch = native_tool("apply_patch")["description"]
        .as_str()
        .unwrap()
        .to_string();
    for invariant in [
        "fileChangeTarget",
        "next model response",
        "never across writes in one batch",
        "Failure/rejection/cancellation/conflict/outcome_unknown never renew",
        "allowedNextActions",
        "waiting_approval/applying/outcome_unknown permit status only",
        "before user-visible narration",
    ] {
        assert!(
            patch.contains(invariant),
            "missing file-change contract: {invariant}"
        );
    }
    let command = native_tool("run_command");
    let cwd = command["parameters"]["properties"]["cwd"]["description"]
        .as_str()
        .unwrap();
    for invariant in [
        "first call",
        "Without workspace",
        "absolute command paths",
        "write=all",
    ] {
        assert!(
            cwd.contains(invariant),
            "missing first-command cwd contract: {invariant}"
        );
    }
    assert!(cwd.contains("backend-recognized command"));
    assert!(cwd.contains("currently activated Skill's explicit Host-owned private directory"));
    let inputs = command["parameters"]["properties"]["inputs"]["description"]
        .as_str()
        .unwrap();
    assert!(inputs.contains("path plus optional mountPath, never source"));
    let runtime_profile = command["parameters"]["properties"]["runtimeProfile"]["description"]
        .as_str()
        .unwrap();
    assert!(runtime_profile.contains("currently activated Skill instructs"));
    assert!(runtime_profile.contains("else omit"));
    assert!(runtime_profile.contains("No guessed profiles, package versions"));
    let image = native_tool("read_image")["description"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(image.contains("Only path") && image.contains("never invent source"));
    let session = native_tool("command_session")["description"]
        .as_str()
        .unwrap()
        .to_string();
    for invariant in ["outcome_unknown", "stop polling", "never replay"] {
        assert!(
            session.contains(invariant),
            "missing unknown-session outcome contract: {invariant}"
        );
    }
    assert!(payload["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| tool["function"]["name"] != "conversation_history"));
    assert_eq!(payload["tools"].as_array().unwrap().len(), 8);
    // Neither native definitions nor their instructions may point back at removed base tools.
    let serialized = payload.to_string();
    for removed in [
        "todo_update",
        "workspace_map",
        "search_files",
        "search_code",
    ] {
        assert!(
            !serialized.contains(removed),
            "stale tool guidance: {removed}"
        );
    }
    assert!(system.contains("never invented parameters/history retrieval"));
    assert!(system.contains("read-only run_command under existing permissions/approvals"));
}

#[test]
fn generic_tool_wording_stays_below_pre_cleanup_budget() {
    // Same one-message fixture, without history, custom instructions or extensions. These
    // pre-cleanup ceilings use the production estimator, not provider-billed token usage.
    for (profile, previous_schema_tokens) in [
        (AgentContextProfile::Full, 9_696),
        (AgentContextProfile::Minimal, 5_200),
    ] {
        let input = wording_input(profile);
        let capabilities =
            prepare_runtime_capabilities(&input, "wording-budget", &[], true, None).unwrap();
        let tools = capabilities.initial_tool_set.stable_definitions();
        let mut request = build_llm_request(input.clone(), tools, None, None, None).unwrap();
        let detector = ContextCapacityDetector::for_model(
            &input.model,
            crate::protocol::AgentApiStyle::OpenAiCompatible,
            tools,
        );
        let costs = detector
            .inspect(
                &mut request.context,
                input.context_window_tokens,
                reserved_output_tokens(&input),
            )
            .context_cost_breakdown();
        assert!(
            costs.tool_schema_tokens < previous_schema_tokens,
            "{profile:?} generic guidance regrew past its pre-cleanup budget: {costs:?}"
        );
        if profile == AgentContextProfile::Minimal {
            // A regression ceiling in production-estimator units, not a vendor token count.
            assert!(
                costs.system_tokens + costs.tool_schema_tokens <= 7_500,
                "minimal fixed guidance grew: {costs:?}"
            );
        }
    }
}
