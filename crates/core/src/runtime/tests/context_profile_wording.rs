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
        "每个子 Agent 都有独立 Run",
        "ref 原样填入 skills_activate.skillRef",
        "完整 Skill 指令及新工具只从下一次模型请求生效",
        "copy-first 工作流",
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
        "never share that ID across writes in one batch",
        "Failure/rejection/cancellation/conflict/outcome_unknown do not renew",
        "allowedNextActions",
        "waiting_approval/applying/outcome_unknown allow only status",
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
    assert!(cwd.contains("activated Skill explicitly supplies a Host-owned private directory"));
    let inputs = command["parameters"]["properties"]["inputs"]["description"]
        .as_str()
        .unwrap();
    assert!(inputs.contains("path and optional mountPath, never source"));
    let runtime_profile = command["parameters"]["properties"]["runtimeProfile"]["description"]
        .as_str()
        .unwrap();
    assert!(runtime_profile.contains("currently activated Skill explicitly instructs"));
    assert!(runtime_profile.contains("otherwise omit, never guess"));
    let image = native_tool("read_image")["description"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(image.contains("Pass only path") && image.contains("never invent source"));
    let session = native_tool("command_session")["description"]
        .as_str()
        .unwrap()
        .to_string();
    for invariant in [
        "outcome_unknown",
        "stop polling",
        "never replay the command",
    ] {
        assert!(
            session.contains(invariant),
            "missing unknown-session outcome contract: {invariant}"
        );
    }
    let history = native_tool("conversation_history");
    let open = history["parameters"]["properties"]["open"]["description"]
        .as_str()
        .unwrap();
    assert!(open.contains("opaque hist_v1_") && open.contains("never modify or invent"));
}
