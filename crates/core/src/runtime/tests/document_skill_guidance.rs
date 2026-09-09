use super::*;
use crate::protocol::AgentContextProfile;
use crate::skills::{AgentSkillDiscoverySnapshot, SkillsService};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::TcpListener;

fn assert_no_document_workflow(text: &str, location: &str) {
    let lower = text.to_ascii_lowercase();
    for forbidden in [
        "office",
        "pdf",
        ".docx",
        ".xlsx",
        ".pptx",
        "builder",
        "电子表格",
        "演示文稿",
    ] {
        assert!(
            !lower.contains(forbidden),
            "document-specific guidance `{forbidden}` leaked into {location}: {text}"
        );
    }
}

fn assert_generic_descriptions(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                if name == "description" {
                    assert_no_document_workflow(value.as_str().unwrap(), "stable Tool description");
                } else {
                    assert_generic_descriptions(value);
                }
            }
        }
        Value::Array(values) => values.iter().for_each(assert_generic_descriptions),
        _ => {}
    }
}

fn system_messages(request: &Value) -> Vec<Value> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "system")
        .cloned()
        .collect()
}

fn message_texts(request: &Value) -> impl Iterator<Item = &str> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
}

fn native_tool<'a>(request: &'a Value, name: &str) -> Option<&'a Value> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == name)
}

#[tokio::test]
async fn document_workflows_enter_only_activated_skill_bodies_in_both_profiles() {
    // Use the actual bundled instructions, but keep execution entirely on a loopback mock
    // provider. A real private Office registry proves that its tools remain activation-gated.
    let skills = SkillsService::new().with_bundled_source().unwrap();
    let catalog = skills.list().unwrap();
    let office_engine =
        crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());

    for profile in [AgentContextProfile::Full, AgentContextProfile::Minimal] {
        let mut baseline_system = None;
        let mut baseline_tools = None;
        for (local_id, reader) in [
            ("documents", Some("office_document")),
            ("spreadsheets", Some("office_spreadsheet")),
            ("presentations", Some("office_presentation")),
            ("pdf", None),
        ] {
            let skill_id = format!("bundled:application:{local_id}");
            let descriptor = catalog
                .skills()
                .iter()
                .find(|skill| skill.id().as_str() == skill_id)
                .unwrap();
            let activated = skills.activate(&[descriptor.selection()]).unwrap();
            let instructions = activated.skills()[0].instructions().to_string();
            let discovery = AgentSkillDiscoverySnapshot::from_descriptors(
                "document-workflow-disclosure-test",
                [descriptor],
                Some(128_000),
                8,
                512 * 1024,
            )
            .unwrap();

            for enabled in [false, true] {
                let case = format!("{profile:?}/{local_id}/enabled={enabled}");
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let activation_ref = discovery.skills[0].activation_ref.clone();
                let server =
                    tokio::spawn(async move {
                        let mut requests = Vec::new();
                        for index in 0..if enabled { 2 } else { 1 } {
                            let (mut stream, _) = listener.accept().await.unwrap();
                            requests.push(read_runtime_test_json_request(&mut stream).await);
                            let activate = enabled && index == 0;
                            let response_message = if activate {
                                json!({
                                    "role": "assistant", "content": null,
                                    "tool_calls": [{
                                        "id": "activate-document-workflow", "type": "function",
                                        "function": {
                                            "name": "skills_activate",
                                            "arguments": json!({
                                                "skillRef": activation_ref,
                                                "reason": "Read the requested specialist workflow"
                                            }).to_string()
                                        }
                                    }]
                                })
                            } else {
                                json!({"role": "assistant", "content": "Ready."})
                            };
                            write_runtime_test_json_response(&mut stream, json!({
                            "choices": [{
                                "message": response_message,
                                "finish_reason": if activate { "tool_calls" } else { "stop" }
                            }]
                        })).await;
                        }
                        requests
                    });
                let entry = discovery.skills[0].clone();
                let skill_instructions = instructions.clone();
                let resolver_calls = Arc::new(AtomicUsize::new(0));
                let calls = Arc::clone(&resolver_calls);
                let resolver: AgentSkillActivationResolver = Arc::new(move |selection| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(selection.skill_id().as_str(), entry.id);
                    assert_eq!(selection.expected_revision().as_str(), entry.revision);
                    let resources = crate::skills::memory_resource_session_for_test(
                        selection.skill_id().clone(),
                        selection.expected_revision().clone(),
                        selection.skill_id().source_id().clone(),
                        Vec::new(),
                    )
                    .unwrap();
                    Ok(AgentResolvedSkillActivation {
                        skill: AgentActivatedSkill {
                            id: entry.id.clone(),
                            name: entry.name.clone(),
                            revision: entry.revision.clone(),
                            source: "bundled:application".to_string(),
                            instructions: skill_instructions.clone(),
                            source_bytes: skill_instructions.len() as u64,
                            resources: None,
                        },
                        resources: Arc::new(resources),
                    })
                });
                let mut input = conversation_context_input(vec![message("user", "你好")]);
                input.api_url = format!("http://{address}/v1/chat/completions");
                input.api_token = "loopback-document-guidance-fixture".to_string();
                input.stream = Some(false);
                input.prompt_preferences =
                    Some(serde_json::from_value(json!({"contextProfile": profile})).unwrap());
                input.skill_discovery = enabled.then(|| discovery.clone());
                let output = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    AgentRuntime::default().send_chat_with_events_and_cancellation(
                        input,
                        Some(format!("document-guidance-{case}")),
                        None,
                        AgentCancellationToken::new(),
                        Some(
                            AgentRuntimeHostServices::new()
                                .with_skill_activation_resolver(resolver)
                                .with_skill_resources(Arc::new(
                                    crate::skills::SkillResourceSession::empty(),
                                ))
                                .with_office_engine(Arc::clone(&office_engine)),
                        ),
                    ),
                )
                .await
                .unwrap_or_else(|_| panic!("native request timed out: {case}"))
                .unwrap_or_else(|error| panic!("native request failed for {case}: {error}"));
                assert_eq!(output.content, "Ready.", "{case}");
                assert_eq!(resolver_calls.load(Ordering::SeqCst), usize::from(enabled));
                let requests = server.await.unwrap();
                let first = &requests[0];
                let system = system_messages(first);
                for message in &system {
                    assert_no_document_workflow(message["content"].as_str().unwrap(), "system");
                }
                assert_generic_descriptions(&first["tools"]);
                assert!(!message_texts(first).any(|text| text.contains(&instructions)));
                assert!(
                    !message_texts(first).any(|text| text.contains("<backend_activated_skill>"))
                );
                let catalogs = message_texts(first)
                    .filter(|text| text.contains("<backend_available_skills>"))
                    .collect::<Vec<_>>();
                assert_eq!(catalogs.len(), usize::from(enabled), "{case}");
                if enabled {
                    assert!(catalogs[0].contains(descriptor.name()));
                    assert!(catalogs[0].contains(descriptor.description()));
                }
                for name in [
                    "office_document",
                    "office_spreadsheet",
                    "office_presentation",
                    "read_word",
                    "read_spreadsheet",
                    "read_presentation",
                ] {
                    assert!(native_tool(first, name).is_none(), "ungated {name}: {case}");
                }
                // Enum values are deliberately preserved API, not specialist instructions.
                let command = &native_tool(first, "run_command").unwrap()["function"];
                assert_eq!(
                    command["parameters"]["properties"]["runtimeProfile"]["enum"],
                    json!(["documents", "spreadsheets", "presentations"])
                );
                assert_eq!(
                    command["parameters"]["properties"]["observe"]["properties"]["kinds"]["items"]
                        ["enum"],
                    json!(["office"])
                );
                if let Some(baseline) = &baseline_system {
                    assert_eq!(
                        baseline, &system,
                        "enablement must not rewrite system: {case}"
                    );
                    assert_eq!(
                        baseline_tools.as_ref().unwrap(),
                        &first["tools"],
                        "enablement must not rewrite stable tools: {case}"
                    );
                } else {
                    baseline_system = Some(system.clone());
                    baseline_tools = Some(first["tools"].clone());
                }

                if !enabled {
                    continue;
                }
                let second = &requests[1];
                assert_eq!(
                    system,
                    system_messages(second),
                    "activation rewrote system: {case}"
                );
                let stable_tools = first["tools"].as_array().unwrap();
                assert!(
                    second["tools"]
                        .as_array()
                        .unwrap()
                        .starts_with(stable_tools),
                    "activation rewrote a stable tool: {case}"
                );
                let bodies = second["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|message| {
                        message["content"]
                            .as_str()
                            .is_some_and(|text| text.contains(&instructions))
                    })
                    .collect::<Vec<_>>();
                assert_eq!(bodies.len(), 1, "missing or duplicate Skill body: {case}");
                assert_eq!(bodies[0]["role"], "user");
                assert!(bodies[0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("<backend_activated_skill>"));
                if let Some(reader) = reader {
                    assert!(
                        native_tool(second, reader).is_some(),
                        "missing activated {reader}: {case}"
                    );
                }
            }
        }
    }
}
