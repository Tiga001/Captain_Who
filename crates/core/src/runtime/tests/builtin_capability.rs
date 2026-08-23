use super::*;
use crate::{
    AgentBuiltinCapabilityActivationApproval, AgentCancellationToken, AgentError, AgentResult,
    AgentToolSafety, BuiltinCapabilityDescriptor, BuiltinCapabilityFuture, BuiltinCapabilityId,
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
    grant: Arc<Mutex<Option<CapabilityGrant>>>,
}

impl BuiltinCapabilityProvider for PayloadCapabilityProvider {
    fn manifests(&self) -> AgentResult<Vec<BuiltinCapabilityManifest>> {
        Ok(vec![self.manifest.clone()])
    }

    fn policy(&self, _: &BuiltinCapabilityId) -> AgentResult<BuiltinCapabilityPolicy> {
        Ok(BuiltinCapabilityPolicy {
            user_allowed: true,
            revision: 11,
        })
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
        grant: Arc::new(Mutex::new(None)),
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
        for _ in 0..2 {
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
    for activated in [false, true] {
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
                policy_revision: 11,
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
    let before = provider_tool_names(&requests[0], api_style);
    assert!(before.contains(&"activate_capability"));
    assert!(!before.contains(&"browser_snapshot"));
    let after = provider_tool_names(&requests[1], api_style);
    assert!(after.contains(&"activate_capability"));
    assert!(after.contains(&"browser_snapshot"));
    assert_eq!(
        provider_tool_schema(&requests[1], api_style, "browser_snapshot"),
        &reviewed_browser_schema(),
        "required/properties/items must survive the Provider-specific projection"
    );
}

#[tokio::test]
async fn reviewed_dynamic_tool_enters_openai_and_anthropic_payload_only_after_grant() {
    run_payload_case(crate::protocol::AgentApiStyle::OpenAiCompatible).await;
    run_payload_case(crate::protocol::AgentApiStyle::AnthropicCompatible).await;
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
