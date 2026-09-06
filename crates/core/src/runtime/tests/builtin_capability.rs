use super::*;
use crate::{
    AgentApprovalDecision, AgentApprovalDecisionStatus, AgentBuiltinCapabilityActivationApproval,
    AgentCancellationToken, AgentError, AgentResult, AgentToolContinuation, AgentToolSafety,
    BuiltinCapabilityDescriptor, BuiltinCapabilityFuture, BuiltinCapabilityId,
    BuiltinCapabilityInvocation, BuiltinCapabilityManifest, BuiltinCapabilityPolicy,
    BuiltinCapabilityProvider, BuiltinCapabilityRuntime, BuiltinCapabilityToolDescriptor,
    CapabilityActivationId, CapabilityGrant,
};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;

#[derive(Clone)]
struct PayloadCapabilityProvider {
    manifest: BuiltinCapabilityManifest,
    policy: Arc<Mutex<BuiltinCapabilityPolicy>>,
    grant: Arc<Mutex<Option<CapabilityGrant>>>,
    invocations: Arc<std::sync::atomic::AtomicUsize>,
}

impl BuiltinCapabilityProvider for PayloadCapabilityProvider {
    fn manifests(&self) -> AgentResult<Vec<BuiltinCapabilityManifest>> {
        Ok(vec![self.manifest.clone()])
    }

    fn policy(&self, _: &BuiltinCapabilityId) -> AgentResult<BuiltinCapabilityPolicy> {
        Ok(self.policy.lock().unwrap().clone())
    }

    fn grant(&self, _: &str, _: &BuiltinCapabilityId) -> AgentResult<Option<CapabilityGrant>> {
        Ok(self.grant.lock().unwrap().clone())
    }

    fn approve_activation(
        &self,
        approval: &AgentBuiltinCapabilityActivationApproval,
    ) -> AgentResult<CapabilityGrant> {
        let capability_id = BuiltinCapabilityId::parse(approval.capability_id.clone())?;
        let grant = CapabilityGrant {
            run_id: approval.run_id.clone(),
            capability_id,
            activation_id: CapabilityActivationId::parse(approval.activation_id.clone())?,
            manifest_digest: approval.manifest_digest.clone(),
            upstream_catalog_digest: self
                .manifest
                .provider_contract
                .upstream_catalog_digest
                .clone(),
            provider_policy_digest: self.manifest.provider_contract.policy_digest.clone(),
            policy_revision: approval.policy_revision,
            created_at: approval.created_at,
            expires_at: approval
                .created_at
                .saturating_add(crate::BUILTIN_CAPABILITY_GRANT_TTL_SECONDS),
        };
        *self.grant.lock().unwrap() = Some(grant.clone());
        Ok(grant)
    }

    fn revoke_grants(&self, _: &BuiltinCapabilityId) -> AgentResult<()> {
        *self.grant.lock().unwrap() = None;
        Ok(())
    }

    fn revoke_activation(&self, _: &CapabilityActivationId, _: &str) -> AgentResult<()> {
        *self.grant.lock().unwrap() = None;
        Ok(())
    }

    fn invoke_authorized<'a>(
        &'a self,
        _: BuiltinCapabilityInvocation,
        _: CapabilityGrant,
        _: AgentCancellationToken,
    ) -> BuiltinCapabilityFuture<'a, Value> {
        self.invocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Err(AgentError::new("not invoked by payload projection test")) })
    }
}

fn payload_capability_runtime() -> (BuiltinCapabilityRuntime, PayloadCapabilityProvider) {
    let manifest = BuiltinCapabilityManifest::new(
        BuiltinCapabilityDescriptor {
            id: BuiltinCapabilityId::parse("browser_automation").unwrap(),
            display_name: "Browser automation".to_string(),
            description: "Control the managed in-app browser".to_string(),
        },
        "builtin.browser_automation.mcp",
        "builtin-browser-automation-v1",
        vec![BuiltinCapabilityToolDescriptor::new(
            "browser.snapshot",
            "browser_snapshot",
            "Read the current managed page",
            reviewed_browser_schema(),
            AgentToolSafety::ReadOnly,
            false,
        )
        .unwrap()],
    )
    .unwrap();
    let provider = PayloadCapabilityProvider {
        manifest,
        policy: Arc::new(Mutex::new(BuiltinCapabilityPolicy {
            user_allowed: true,
            revision: 11,
        })),
        grant: Arc::new(Mutex::new(None)),
        invocations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    (
        BuiltinCapabilityRuntime::new(Arc::new(provider.clone())).unwrap(),
        provider,
    )
}

fn reviewed_browser_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target": { "type": "string" },
            "options": {
                "type": "object",
                "properties": {
                    "wait_for": { "type": "string" },
                    "mode": { "type": "string", "enum": ["visible", "hidden"] },
                    "marker": { "const": "fixture" },
                    "attempts": { "type": "integer", "minimum": 1, "maximum": 5 },
                    "label": { "type": "string", "minLength": 1, "maxLength": 32 },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "tuple": {
                        "type": "array",
                        "prefixItems": [
                            { "type": "string" },
                            { "type": "number" }
                        ],
                        "items": false
                    },
                    "selector": {
                        "oneOf": [
                            { "type": "string" },
                            {
                                "type": "object",
                                "properties": { "ref": { "type": "string" } },
                                "required": ["ref"],
                                "additionalProperties": false
                            }
                        ]
                    },
                    "timeout": {
                        "anyOf": [
                            { "type": "number", "minimum": 0 },
                            { "type": "null" }
                        ]
                    },
                    "rules": {
                        "allOf": [
                            { "type": "object" },
                            {
                                "properties": { "strict": { "type": "boolean" } },
                                "required": ["strict"],
                                "additionalProperties": false
                            }
                        ]
                    }
                },
                "required": ["wait_for"],
                "additionalProperties": false
            }
        },
        "required": ["target", "options"],
        "additionalProperties": false
    })
}

fn provider_tool_names(payload: &Value, api_style: crate::protocol::AgentApiStyle) -> Vec<&str> {
    payload["tools"]
        .as_array()
        .expect("provider payload tools")
        .iter()
        .map(|tool| match api_style {
            crate::protocol::AgentApiStyle::OpenAiCompatible => tool["function"]["name"]
                .as_str()
                .expect("OpenAI function name"),
            crate::protocol::AgentApiStyle::AnthropicCompatible => {
                tool["name"].as_str().expect("Anthropic tool name")
            }
        })
        .collect()
}

fn provider_tool_schema<'a>(
    payload: &'a Value,
    api_style: crate::protocol::AgentApiStyle,
    name: &str,
) -> &'a Value {
    let tool = payload["tools"]
        .as_array()
        .expect("provider payload tools")
        .iter()
        .find(|tool| match api_style {
            crate::protocol::AgentApiStyle::OpenAiCompatible => {
                tool["function"]["name"].as_str() == Some(name)
            }
            crate::protocol::AgentApiStyle::AnthropicCompatible => {
                tool["name"].as_str() == Some(name)
            }
        })
        .expect("reviewed tool in provider payload");
    match api_style {
        crate::protocol::AgentApiStyle::OpenAiCompatible => &tool["function"]["parameters"],
        crate::protocol::AgentApiStyle::AnthropicCompatible => &tool["input_schema"],
    }
}

async fn run_payload_case(api_style: crate::protocol::AgentApiStyle) {
    let (runtime, provider) = payload_capability_runtime();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_for_server = Arc::clone(&captured);
    let server = tokio::spawn(async move {
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            captured_for_server.lock().unwrap().push(request);
            let response = match api_style {
                crate::protocol::AgentApiStyle::OpenAiCompatible => json!({
                    "choices": [{
                        "message": {"role": "assistant", "content": "done"},
                        "finish_reason": "stop"
                    }]
                }),
                crate::protocol::AgentApiStyle::AnthropicCompatible => json!({
                    "content": [{"type": "text", "text": "done"}],
                    "stop_reason": "end_turn"
                }),
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let run_id = "builtin-capability-provider-payload-run";
    for (allowed, revision, activated) in [
        (false, 11, false),
        (true, 12, false),
        (true, 12, true),
        (false, 13, false),
        (true, 14, false),
    ] {
        *provider.policy.lock().unwrap() = BuiltinCapabilityPolicy {
            user_allowed: allowed,
            revision,
        };
        if activated {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let manifest = runtime.manifests()[0].clone();
            *provider.grant.lock().unwrap() = Some(CapabilityGrant {
                run_id: run_id.to_string(),
                capability_id: manifest.descriptor.id,
                activation_id: CapabilityActivationId::generate(),
                manifest_digest: manifest.manifest_digest.clone(),
                upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
                provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
                policy_revision: revision,
                created_at: now,
                expires_at: now + 60,
            });
        }
        let mut input = conversation_context_input(vec![message("user", "Inspect the page")]);
        input.api_url = match api_style {
            crate::protocol::AgentApiStyle::OpenAiCompatible => {
                format!("http://{address}/v1/chat/completions")
            }
            crate::protocol::AgentApiStyle::AnthropicCompatible => {
                format!("http://{address}/v1/messages")
            }
        };
        input.api_token = "test-token".to_string();
        input.api_style = Some(api_style);
        input.stream = Some(false);
        freeze_runtime_test_generic_provider(&mut input, "builtin-capability-payload");
        let output = AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some(run_id.to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_builtin_capabilities(runtime.clone())),
            )
            .await
            .unwrap();
        assert_eq!(output.status, AgentRunStatus::Completed);
    }
    server.await.unwrap();

    let requests = captured.lock().unwrap();
    for index in [0, 3] {
        let disabled = provider_tool_names(&requests[index], api_style);
        assert!(!disabled.contains(&"activate_capability"));
        assert!(!disabled.contains(&"browser_snapshot"));
        assert!(disabled.iter().all(|name| !name.starts_with("browser_")));
        let payload = serde_json::to_string(&requests[index]).unwrap();
        assert!(payload.contains(r#"userAllowed\":false"#));
        assert!(!payload.contains("Control the managed in-app browser"));
        assert!(!payload.contains("Enabled built-in capabilities"));
        assert!(!payload.contains("policyRevision"));
    }
    let before = provider_tool_names(&requests[1], api_style);
    assert!(before.contains(&"activate_capability"));
    assert!(!before.contains(&"browser_snapshot"));
    let after = provider_tool_names(&requests[2], api_style);
    assert!(after.contains(&"activate_capability"));
    assert!(after.contains(&"browser_snapshot"));
    assert_eq!(
        provider_tool_schema(&requests[2], api_style, "browser_snapshot"),
        &reviewed_browser_schema(),
        "required/properties/items must survive the Provider-specific projection"
    );
    let reenabled = provider_tool_names(&requests[4], api_style);
    assert!(reenabled.contains(&"activate_capability"));
    assert!(!reenabled.contains(&"browser_snapshot"));
    assert!(serde_json::to_string(&requests[4])
        .unwrap()
        .contains("awaiting approval"));
    assert!(!serde_json::to_string(provider_tool_schema(
        &requests[4],
        api_style,
        "activate_capability"
    ))
    .unwrap()
    .contains("browser"));

    let system_prefix = |payload: &Value| match api_style {
        crate::protocol::AgentApiStyle::OpenAiCompatible => payload["messages"][0].clone(),
        crate::protocol::AgentApiStyle::AnthropicCompatible => payload["system"].clone(),
    };
    let prefix = system_prefix(&requests[0]);
    assert!(!prefix.is_null());
    for request in requests.iter().skip(1) {
        assert_eq!(
            prefix,
            system_prefix(request),
            "switches must not alter the stable system prefix"
        );
    }
}

#[tokio::test]
async fn reviewed_dynamic_tool_enters_openai_and_anthropic_payload_only_after_grant() {
    run_payload_case(crate::protocol::AgentApiStyle::OpenAiCompatible).await;
    run_payload_case(crate::protocol::AgentApiStyle::AnthropicCompatible).await;
}

#[tokio::test]
async fn reenabled_browser_waits_for_real_approval_and_resumes_with_fresh_authority() {
    let (runtime, provider) = payload_capability_runtime();
    let run_id = "builtin-reenabled-approval-run";
    let manifest = &runtime.manifests()[0];
    let now = crate::builtin_capabilities::unix_timestamp();
    let old_approval = AgentBuiltinCapabilityActivationApproval {
        action_id: uuid::Uuid::new_v4().to_string(),
        activation_id: CapabilityActivationId::generate().as_str().to_string(),
        run_id: run_id.to_string(),
        call_id: "previous-browser-activation".to_string(),
        capability_id: manifest.descriptor.id.as_str().to_string(),
        display_name: manifest.descriptor.display_name.clone(),
        reason: "Previously reviewed browser access".to_string(),
        manifest_digest: manifest.manifest_digest.clone(),
        policy_revision: 11,
        created_at: now,
        expires_at: now + crate::BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
        approval_status: AgentApprovalStatus::Approved,
    };
    runtime.approve_activation(&old_approval).unwrap();
    *provider.policy.lock().unwrap() = BuiltinCapabilityPolicy {
        user_allowed: false,
        revision: 12,
    };
    assert!(runtime.approve_activation(&old_approval).is_err());
    *provider.policy.lock().unwrap() = BuiltinCapabilityPolicy {
        user_allowed: true,
        revision: 13,
    };
    assert!(runtime.approve_activation(&old_approval).is_err());
    assert!(runtime
        .live_grant(run_id, &manifest.descriptor.id)
        .unwrap()
        .is_none());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let api_style = crate::protocol::AgentApiStyle::OpenAiCompatible;
        let (mut stream, _) = listener.accept().await.unwrap();
        let awaiting = read_runtime_test_json_request(&mut stream).await;
        let names = provider_tool_names(&awaiting, api_style);
        assert!(names.contains(&"activate_capability"));
        assert!(!names.contains(&"browser_snapshot"));
        assert!(serde_json::to_string(&awaiting)
            .unwrap()
            .contains("awaiting approval"));
        write_runtime_test_json_response(&mut stream, json!({
            "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [{
                "id": "fresh-browser-activation", "type": "function", "function": {
                    "name": "activate_capability",
                    "arguments": r#"{"capability":"browser_automation","reason":"Inspect the reviewed browser"}"#
                }
            }]}, "finish_reason": "tool_calls"}]
        })).await;

        let (mut stream, _) = listener.accept().await.unwrap();
        let approved = read_runtime_test_json_request(&mut stream).await;
        assert!(provider_tool_names(&approved, api_style).contains(&"browser_snapshot"));
        assert_eq!(awaiting["messages"][0], approved["messages"][0]);
        assert!(serde_json::to_string(&approved)
            .unwrap()
            .contains("activate_capability"));
        write_runtime_test_json_response(&mut stream, json!({
            "choices": [{"message": {"role": "assistant", "content": "Access approved."}, "finish_reason": "stop"}]
        })).await;
    });
    let mut input = conversation_context_input(vec![message("user", "Inspect the page")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    freeze_runtime_test_generic_provider(&mut input, "builtin-reenabled-approval");
    let host = AgentRuntimeHostServices::new().with_builtin_capabilities(runtime.clone());
    let waiting = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input.clone(),
            Some(run_id.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(host.clone()),
        )
        .await
        .unwrap();
    assert_eq!(waiting.status, AgentRunStatus::WaitingForApproval);
    let (mut approval, checkpoint) = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired {
                action, checkpoint, ..
            } => match action.as_ref() {
                AgentProposedAction::BuiltinCapabilityActivation { approval } => {
                    Some(((**approval).clone(), (**checkpoint).clone()))
                }
                _ => None,
            },
            _ => None,
        })
        .expect("fresh typed activation approval and checkpoint");
    assert_eq!(approval.approval_status, AgentApprovalStatus::Required);
    assert_eq!(approval.policy_revision, 13);
    assert!(runtime
        .live_grant(run_id, &manifest.descriptor.id)
        .unwrap()
        .is_none());
    assert!(!checkpoint
        .tool_set
        .exposed_tool_names
        .iter()
        .any(|name| name == "browser_snapshot"));

    // The Host settles the exact approval before resuming; the feature switch alone did not.
    approval.approval_status = AgentApprovalStatus::Approved;
    runtime.approve_activation(&approval).unwrap();
    let pending_call = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .find(|call| call.id == approval.call_id)
        .unwrap()
        .clone();
    input.messages.clear();
    input.resume_checkpoint = Some(checkpoint);
    input.approval_decision = Some(AgentApprovalDecision {
        action_id: approval.action_id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: pending_call.id,
            tool: pending_call.name,
            args: pending_call.args,
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: crate::builtin_capability_activation_result(
            &approval,
            crate::CapabilityActivationState::Active,
            None,
        ),
    });
    let completed = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(run_id.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(host),
        )
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(completed.status, AgentRunStatus::Completed);
    assert_eq!(
        provider
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn disable_after_model_dispatch_rejects_late_call_and_cleans_next_request() {
    let (runtime, provider) = payload_capability_runtime();
    let run_id = "builtin-late-call-run";
    let manifest = &runtime.manifests()[0];
    let now = crate::builtin_capabilities::unix_timestamp();
    *provider.grant.lock().unwrap() = Some(CapabilityGrant {
        run_id: run_id.to_string(),
        capability_id: manifest.descriptor.id.clone(),
        activation_id: CapabilityActivationId::generate(),
        manifest_digest: manifest.manifest_digest.clone(),
        upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
        provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
        policy_revision: 11,
        created_at: now,
        expires_at: now + 60,
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_provider = provider.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let first = read_runtime_test_json_request(&mut stream).await;
        assert!(
            provider_tool_names(&first, crate::protocol::AgentApiStyle::OpenAiCompatible)
                .contains(&"browser_snapshot")
        );
        *server_provider.policy.lock().unwrap() = BuiltinCapabilityPolicy {
            user_allowed: false,
            revision: 12,
        };
        write_runtime_test_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {"role":"assistant", "content":null, "tool_calls":[{
                        "id":"late-browser-call", "type":"function", "function":{
                            "name":"browser_snapshot",
                            "arguments":r#"{"target":"current","options":{"wait_for":"ready"}}"#,
                        }
                    }]},
                    "finish_reason":"tool_calls"
                }]
            }),
        )
        .await;
        let (mut stream, _) = listener.accept().await.unwrap();
        let second = read_runtime_test_json_request(&mut stream).await;
        let names = provider_tool_names(&second, crate::protocol::AgentApiStyle::OpenAiCompatible);
        assert!(!names.contains(&"browser_snapshot"));
        assert!(!names.contains(&"activate_capability"));
        let next_payload = serde_json::to_string(&second).unwrap();
        assert!(next_payload.contains(r#"userAllowed\":false"#));
        assert!(!next_payload.contains("Enabled built-in capabilities"));
        assert!(!next_payload.contains("Control the managed in-app browser"));
        assert!(
            next_payload.contains("browser_snapshot"),
            "the historical call must remain in context"
        );
        assert_eq!(first["messages"][0], second["messages"][0]);
        write_runtime_test_json_response(&mut stream, json!({
            "choices":[{"message":{"role":"assistant","content":"Capability disabled."},"finish_reason":"stop"}]
        })).await;
    });
    let mut input = conversation_context_input(vec![message("user", "Inspect the page")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    freeze_runtime_test_generic_provider(&mut input, "builtin-late-call");
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(run_id.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_builtin_capabilities(runtime)),
        )
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(output.status, AgentRunStatus::Completed);
    assert_eq!(
        provider
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(output.events.iter().any(|event| matches!(event, AgentEvent::ToolResult { result, .. } if result.tool == "browser_snapshot" && !result.ok)));
    assert!(!output
        .events
        .iter()
        .any(|event| matches!(event, AgentEvent::ApprovalRequired { .. })));
}

#[tokio::test]
async fn automatic_builtin_activation_uses_typed_host_action_without_waiting_for_approval() {
    let (runtime, _) = payload_capability_runtime();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for response in [
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "activate-browser-call",
                            "type": "function",
                            "function": {
                                "name": "activate_capability",
                                "arguments": "{\"capability\":\"browser_automation\",\"reason\":\"Open the reviewed browser\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
            json!({
                "choices": [{
                    "message": {"role": "assistant", "content": "activated"},
                    "finish_reason": "stop"
                }]
            }),
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_runtime_test_json_request(&mut stream).await;
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let host_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor_runtime = runtime.clone();
    let executor_calls = Arc::clone(&host_calls);
    let host_executor: AgentHostActionExecutor =
        Arc::new(move |action, checkpoint, cancellation| {
            assert!(!cancellation.is_cancelled());
            assert!(
                checkpoint.is_none(),
                "activation has no external dispatch payload"
            );
            let AgentProposedAction::BuiltinCapabilityActivation { approval } = action else {
                return Err(AgentError::new("expected typed built-in activation"));
            };
            assert_eq!(approval.approval_status, AgentApprovalStatus::Approved);
            executor_runtime.approve_activation(&approval)?;
            executor_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::builtin_capability_activation_result(
                &approval,
                crate::CapabilityActivationState::Active,
                None,
            ))
        });

    let mut input = conversation_context_input(vec![message("user", "Open the browser")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.context = Some(crate::AgentRunContext {
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: crate::AgentPermissions {
            builtin_execution: crate::AgentBuiltinExecutionPermission::AutoApprove,
            ..crate::AgentPermissions::default()
        },
        collaboration_identity: None,
    });
    freeze_runtime_test_generic_provider(&mut input, "builtin-auto-activation");
    let storage_fixture = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        crate::storage::service::StorageService::open(&storage_fixture.path().join("core.sqlite"))
            .unwrap(),
    );
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("builtin-auto-activation-run".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_builtin_capabilities(runtime)
                    .with_host_actions(host_executor, storage),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(output.status, AgentRunStatus::Completed);
    assert_eq!(host_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!output
        .events
        .iter()
        .any(|event| matches!(event, AgentEvent::ApprovalRequired { .. })));
}

#[test]
fn builtin_auto_route_requires_both_effective_permission_and_call_level_approval() {
    let activation = AgentToolIdentity::RuntimeExtension {
        extension_id: crate::builtin_capabilities::BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID
            .to_string(),
        tool_name: crate::builtin_capabilities::ACTIVATE_CAPABILITY_TOOL_NAME.to_string(),
    };
    assert!(!auto_executes_builtin_prepared_action(
        crate::AgentBuiltinExecutionPermission::RequireApproval,
        true,
        &activation,
    ));
    assert!(auto_executes_builtin_prepared_action(
        crate::AgentBuiltinExecutionPermission::AutoApprove,
        true,
        &activation,
    ));

    let sensitive = AgentToolIdentity::BuiltinCapability {
        capability_id: "browser_automation".into(),
        managed_mcp_id: "builtin.browser_automation.mcp".into(),
        package_name: "fixture".into(),
        package_version: "1.0.0".into(),
        upstream_catalog_digest: "catalog".into(),
        policy_digest: "policy".into(),
        manifest_digest: "manifest".into(),
        tool_id: "browser_file_upload".into(),
        raw_name: "browser_file_upload".into(),
        model_name: "browser_file_upload".into(),
        upstream_schema_digest: "upstream".into(),
        host_overlay_digest: "overlay".into(),
        host_input_schema_digest: "input".into(),
    };
    assert!(auto_executes_builtin_prepared_action(
        crate::AgentBuiltinExecutionPermission::AutoApprove,
        true,
        &sensitive,
    ));
    assert!(
        !auto_executes_builtin_prepared_action(
            crate::AgentBuiltinExecutionPermission::AutoApprove,
            false,
            &sensitive,
        ),
        "a Dynamic upload/drop call with benign arguments must not enter the sensitive grant path"
    );
}
