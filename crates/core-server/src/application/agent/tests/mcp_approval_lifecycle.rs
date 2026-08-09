use super::*;
use mycopilot_core::{
    mcp_normalized_input_schema_identity, mcp_tool_arguments_digest, AgentMcpApprovalMode,
    AgentMcpServerScope, AgentMcpToolApproval, AgentMcpToolInvocationIdentity,
    AgentMcpToolProvenance, McpAgentToolAnnotations, McpAgentToolDescriptor,
    McpApprovedToolInvocation, McpToolApprovalRequest, McpToolCatalogContext, McpToolContentBlock,
    McpToolInvocationFuture, McpToolInvocationResult, McpToolInvoker,
    MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const ARGUMENT_CANARY: &str = "MCP_NEUTRAL_ARGUMENT_CANARY_NOT_DURABLE";
const RESULT_CANARY: &str = "MCP_RESULT_CANARY_LIVE_MODEL_ONLY";
const REASONING_CANARY: &str = "DEEPSEEK_RAW_REASONING_CANARY_BYTE_EXACT_PRIVATE_REPLAY_7f4a1c";

#[derive(Clone, Copy)]
struct DurableApprovalStartupInspector;

impl McpApprovalStartupInspector for DurableApprovalStartupInspector {
    fn inspect_startup_payload(
        &self,
        _approval: &AgentMcpToolApproval,
    ) -> McpApprovalStartupPayloadState {
        McpApprovalStartupPayloadState::DurableAvailable
    }
}

#[derive(Clone, Copy, Default)]
enum ApprovalInvocationBehavior {
    #[default]
    Success,
    ServerToolError,
    BareCancellation,
    TruncatedSuccess,
    WaitForCancellation,
}

#[derive(Default)]
struct ApprovalLifecycleInvoker {
    prepared: Mutex<HashMap<String, (AgentMcpToolApproval, Value)>>,
    invocation_count: AtomicU64,
    descriptor: Mutex<Option<McpAgentToolDescriptor>>,
    behavior: ApprovalInvocationBehavior,
    invocation_started: AtomicBool,
    invocation_started_notify: tokio::sync::Notify,
}

impl ApprovalLifecycleInvoker {
    fn with_descriptor(descriptor: McpAgentToolDescriptor) -> Arc<Self> {
        Self::with_behavior(descriptor, ApprovalInvocationBehavior::Success)
    }

    fn with_behavior(
        descriptor: McpAgentToolDescriptor,
        behavior: ApprovalInvocationBehavior,
    ) -> Arc<Self> {
        Arc::new(Self {
            prepared: Mutex::new(HashMap::new()),
            invocation_count: AtomicU64::new(0),
            descriptor: Mutex::new(Some(descriptor)),
            behavior,
            invocation_started: AtomicBool::new(false),
            invocation_started_notify: tokio::sync::Notify::new(),
        })
    }

    async fn wait_until_invocation_started(&self) {
        loop {
            if self.invocation_started.load(Ordering::SeqCst) {
                return;
            }
            self.invocation_started_notify.notified().await;
        }
    }
}

impl McpToolInvoker for ApprovalLifecycleInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<McpAgentToolDescriptor>> {
        Ok(self
            .descriptor
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .into_iter()
            .collect())
    }

    fn prepare_approval(
        &self,
        request: McpToolApprovalRequest,
    ) -> AgentResult<AgentMcpToolApproval> {
        let (approval, arguments, _) = request.into_parts();
        if mcp_tool_arguments_digest(&arguments)? != approval.identity.arguments_digest {
            return Err(AgentError::new("test approval argument binding changed"));
        }
        self.prepared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                approval.identity.invocation_id.clone(),
                (approval.clone(), arguments),
            );
        Ok(approval)
    }

    fn invalidate_prepared_approval(
        &self,
        identity: &AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        self.prepared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&identity.invocation_id);
        Ok(())
    }

    fn revalidate_approved(&self, approval: &AgentMcpToolApproval) -> AgentResult<()> {
        let prepared = self
            .prepared
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some((stored, arguments)) = prepared.get(&approval.identity.invocation_id) else {
            return Err(AgentError::new("test approval payload unavailable"));
        };
        if stored != approval
            || mcp_tool_arguments_digest(arguments)? != approval.identity.arguments_digest
        {
            return Err(AgentError::new("test approval binding changed"));
        }
        Ok(())
    }

    fn invoke_approved<'a>(
        &'a self,
        invocation: McpApprovedToolInvocation,
        cancellation: AgentCancellationToken,
    ) -> McpToolInvocationFuture<'a> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(AgentError::cancelled());
            }
            let prepared = self
                .prepared
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&invocation.approval.identity.invocation_id);
            let Some((stored, arguments)) = prepared else {
                return Err(AgentError::new(
                    "test approval payload was consumed or unavailable",
                ));
            };
            if stored != invocation.approval || arguments != json!({"value": ARGUMENT_CANARY}) {
                return Err(AgentError::new("test approval payload changed"));
            }
            self.invocation_count.fetch_add(1, Ordering::SeqCst);
            self.invocation_started.store(true, Ordering::SeqCst);
            self.invocation_started_notify.notify_waiters();
            if matches!(
                self.behavior,
                ApprovalInvocationBehavior::WaitForCancellation
            ) {
                cancellation.cancelled().await;
                return Err(AgentError::cancelled());
            }
            if matches!(self.behavior, ApprovalInvocationBehavior::BareCancellation) {
                return Err(AgentError::cancelled());
            }
            Ok(McpToolInvocationResult {
                content: vec![McpToolContentBlock::Text {
                    text: RESULT_CANARY.to_string(),
                }],
                structured_content: Some(json!({"status": "fixture-complete"})),
                is_error: matches!(self.behavior, ApprovalInvocationBehavior::ServerToolError),
                truncated_at_source: matches!(
                    self.behavior,
                    ApprovalInvocationBehavior::TruncatedSuccess
                ),
            })
        })
    }
}

fn lifecycle_descriptor() -> McpAgentToolDescriptor {
    let input_schema = json!({
        "type": "object",
        "properties": {
            "value": {"type": "string"}
        },
        "required": ["value"]
    });
    let normalized =
        mcp_normalized_input_schema_identity("mcp__approval_fixture__echo", &input_schema).unwrap();
    assert_eq!(
        normalized.normalizer_version,
        MCP_INPUT_SCHEMA_NORMALIZER_VERSION
    );
    McpAgentToolDescriptor {
        provenance: AgentMcpToolProvenance {
            server_id: uuid::Uuid::new_v4().to_string(),
            scope: AgentMcpServerScope::User,
            raw_tool_name: "echo".to_string(),
            model_tool_name: "mcp__approval_fixture__echo".to_string(),
            config_epoch: "bf616f04-d3ec-4bd7-825f-731a9f0892f4".to_string(),
            registry_revision: 11,
            config_digest: "1".repeat(64),
            catalog_generation: 1,
            catalog_digest: "2".repeat(64),
            catalog_schema_digest: "3".repeat(64),
            schema_digest: normalized.schema_digest,
            schema_normalizer_version: normalized.normalizer_version,
        },
        approval_mode: AgentMcpApprovalMode::Prompt,
        server_display_name: "Owned approval fixture".to_string(),
        description: Some("Repository-owned approval lifecycle fixture".to_string()),
        input_schema,
        output_schema: None,
        annotations: McpAgentToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        },
    }
}

fn provider_mcp_tool_arguments(request: &Value, tool_name: &str) -> Value {
    let encoded = request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .find_map(|call| {
            (call["function"]["name"].as_str() == Some(tool_name))
                .then(|| call["function"]["arguments"].as_str())
                .flatten()
        })
        .expect("provider request must retain the MCP Tool call arguments");
    serde_json::from_str(encoded).expect("provider MCP Tool arguments must remain valid JSON")
}

fn save_deepseek_approval_provider(
    storage: &StorageService,
    model_id: &str,
    api_url: &str,
    reasoning_mode: mycopilot_core::ReasoningMode,
) {
    let mut profile = mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    profile.reasoning.mode = reasoning_mode;
    storage
        .save_model_settings(ModelSettingsRecord {
            api_url: api_url.to_string(),
            api_token: "fixed-test-model-token".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: model_id.to_string(),
                display_name: "DeepSeek approval continuation fixture".to_string(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                provider_profile_config: Some(profile),
                input_price: "0".to_string(),
                output_price: "0".to_string(),
                enabled: true,
            }],
        })
        .unwrap();
}

#[derive(Clone, Copy)]
enum RestartedApprovalDecision {
    Approve,
    Reject,
}

async fn run_deepseek_missing_reasoning_restart(
    reasoning_mode: mycopilot_core::ReasoningMode,
    decision: RestartedApprovalDecision,
    response_reasoning: Option<&'static str>,
) -> (Value, u64) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let first_request = read_json_request(&mut first).await;
        write_tool_call_stream_with_reasoning(&mut first, response_reasoning).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "Restarted approval completed.").await;
        drop(second);
        (first_request, second_request)
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("deepseek-approval-restart.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let model_id = "deepseek-approval-restart-model";
    save_deepseek_approval_provider(
        &storage,
        model_id,
        &format!("http://{address}/v1/chat/completions"),
        reasoning_mode,
    );
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let vault = Arc::new(
        mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&storage),
            credentials.clone(),
        )
        .unwrap(),
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let service =
        AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
            Arc::clone(&storage),
            Some(vault),
        )
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-deepseek-approval-restart".to_string()),
                project_id: None,
                model_id: model_id.to_string(),
                context_window_indicator_enabled: response_reasoning.is_some(),
                content: "Call the approval fixture, then wait.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-deepseek-approval-restart".to_string()),
                assistant_message_id: Some("assistant-deepseek-approval-restart".to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();
    let (approval_required, mut pre_restart_events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "approval_required"
        })
        .await;
    pre_restart_events.push(approval_required);
    let (waiting, more_pre_restart_events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
                && notification["params"]["status"] == "waiting_for_approval"
        })
        .await;
    pre_restart_events.extend(more_pre_restart_events);
    pre_restart_events.push(waiting);
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 0);

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let (active_envelopes, encrypted_envelopes): (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*),
                    SUM(CASE WHEN ciphertext IS NOT NULL AND length(ciphertext) > 16 THEN 1 ELSE 0 END)
             FROM provider_continuations
             WHERE conversation_id = ?1 AND state = 'active'",
            ["conversation-deepseek-approval-restart"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((active_envelopes, encrypted_envelopes), (1, 1));
    let persisted_input: String = connection
        .query_row(
            "SELECT agent_input_json FROM agent_pending_actions WHERE run_id = ?1",
            [&turn.run_id],
            |row| row.get(0),
        )
        .unwrap();
    if let Some(reasoning) = response_reasoning {
        assert!(!persisted_input.contains(reasoning));
        assert!(pre_restart_events
            .iter()
            .all(|event| !event.to_string().contains(reasoning)));
        let trace = storage
            .get_conversation_turn_trace("assistant-deepseek-approval-restart")
            .unwrap()
            .unwrap();
        assert!(!serde_json::to_string(&trace).unwrap().contains(reasoning));
        if let Some(model_log) = storage
            .get_conversation_model_context_log("assistant-deepseek-approval-restart")
            .unwrap()
        {
            assert!(!serde_json::to_string(&model_log)
                .unwrap()
                .contains(reasoning));
        }
        assert_files_do_not_contain(&database_path, reasoning.as_bytes());
    }
    let persisted_input: Value = serde_json::from_str(&persisted_input).unwrap();
    assert_eq!(
        persisted_input["resumeCheckpoint"]["providerContinuationRefs"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    drop(connection);
    drop(notifications);
    drop(receiver);
    drop(service);
    drop(storage);

    let reopened_storage = Arc::new(StorageService::open(&database_path).unwrap());
    let reopened_vault = Arc::new(
        mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&reopened_storage),
            credentials,
        )
        .unwrap(),
    );
    let restarted =
        AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
            Arc::clone(&reopened_storage),
            Some(reopened_vault),
        )
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let pending = restarted.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let (restart_notifications, mut restart_receiver) = tokio::sync::mpsc::unbounded_channel();
    match decision {
        RestartedApprovalDecision::Approve => {
            restarted
                .approve_action(&turn.run_id, &pending[0].action_id, restart_notifications)
                .unwrap();
        }
        RestartedApprovalDecision::Reject => {
            restarted
                .reject_action(
                    &turn.run_id,
                    &pending[0].action_id,
                    Some("Do not run this tool.".to_string()),
                    restart_notifications,
                )
                .unwrap();
        }
    }
    let (done, restart_events) =
        wait_for_notification_matching_with_seen(&mut restart_receiver, |notification| {
            notification["params"]["type"] == "done"
        })
        .await;
    assert_eq!(done["params"]["status"], "completed");

    let (first_request, second_request) = model_server.await.unwrap();
    assert!(first_request["messages"]
        .as_array()
        .is_some_and(|messages| messages
            .iter()
            .all(|message| { message.get("reasoning_content").is_none() })));
    let replayed_provider_call = second_request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .find(|call| call["id"] == "mcp-approval-provider-call")
        .expect("restart must restore the original grouped provider Tool call identity");
    assert_eq!(
        replayed_provider_call["function"]["name"],
        "mcp__approval_fixture__echo"
    );
    if let Some(reasoning) = response_reasoning {
        let replayed_assistant = second_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| {
                message["role"] == "assistant"
                    && message["tool_calls"].as_array().is_some_and(|calls| {
                        calls
                            .iter()
                            .any(|call| call["id"] == "mcp-approval-provider-call")
                    })
            })
            .expect("restart must replay the provider Assistant turn");
        assert_eq!(
            replayed_assistant["reasoning_content"].as_str(),
            Some(reasoning),
            "raw reasoning must be replayed byte-for-byte"
        );
        assert!(restart_events
            .iter()
            .all(|event| !event.to_string().contains(reasoning)));
        assert_files_do_not_contain(&database_path, reasoning.as_bytes());

        let runtime_continuation_tokens = restart_events
            .iter()
            .filter(|event| event["params"]["type"] == "context_window_updated")
            .filter_map(|event| {
                event["params"]["snapshot"]["costBreakdown"]["providerContinuationTokens"].as_u64()
            })
            .max()
            .expect("resumed real run must publish exact continuation accounting");
        assert!(runtime_continuation_tokens > 0);
        let preview = restarted
            .get_context_window_snapshot(AgentContextWindowSnapshotInput {
                conversation_id: Some("conversation-deepseek-approval-restart".to_string()),
                project_id: None,
                model_id: "deepseek-approval-restart-model".to_string(),
                max_tokens: Some(1_024),
                prompt_preferences: None,
                permissions: Default::default(),
                skills: Vec::new(),
            })
            .unwrap()
            .snapshot
            .unwrap();
        assert_eq!(
            preview.cost_breakdown.provider_continuation_tokens, runtime_continuation_tokens,
            "Host preview and real resumed request must hydrate the same private continuation"
        );
    }
    (
        second_request,
        invoker.invocation_count.load(Ordering::SeqCst),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deepseek_disabled_tool_turn_without_reasoning_survives_restart_and_approval() {
    let (second_request, invocation_count) = run_deepseek_missing_reasoning_restart(
        mycopilot_core::ReasoningMode::Disabled,
        RestartedApprovalDecision::Approve,
        None,
    )
    .await;
    assert_eq!(invocation_count, 1);
    assert!(serde_json::to_string(&second_request)
        .unwrap()
        .contains(RESULT_CANARY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deepseek_provider_default_tool_turn_without_reasoning_survives_restart_and_rejection() {
    let (second_request, invocation_count) = run_deepseek_missing_reasoning_restart(
        mycopilot_core::ReasoningMode::ProviderDefault,
        RestartedApprovalDecision::Reject,
        None,
    )
    .await;
    assert_eq!(invocation_count, 0);
    let serialized = serde_json::to_string(&second_request).unwrap();
    assert!(serialized.contains("Do not run this tool."));
    assert!(!serialized.contains(RESULT_CANARY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deepseek_enabled_reasoning_restarts_privately_and_replays_byte_exact_with_budget_parity() {
    let (second_request, invocation_count) = run_deepseek_missing_reasoning_restart(
        mycopilot_core::ReasoningMode::Enabled,
        RestartedApprovalDecision::Approve,
        Some(REASONING_CANARY),
    )
    .await;
    assert_eq!(invocation_count, 1);
    assert!(second_request.to_string().contains(REASONING_CANARY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restarted_approval_rejects_swapped_valid_provider_refs_before_dispatch() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _request = read_json_request(&mut stream).await;
            write_tool_call_stream(&mut stream).await;
        }
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("deepseek-swapped-refs.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let model_id = "deepseek-swapped-ref-model";
    save_deepseek_approval_provider(
        &storage,
        model_id,
        &format!("http://{address}/v1/chat/completions"),
        mycopilot_core::ReasoningMode::Disabled,
    );
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let vault = Arc::new(
        mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&storage),
            credentials.clone(),
        )
        .unwrap(),
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let service =
        AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
            Arc::clone(&storage),
            Some(vault),
        )
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);

    let mut run_ids = Vec::new();
    for suffix in ["a", "b"] {
        let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let turn = service
            .start_conversation_turn(
                AgentConversationTurnInput {
                    conversation_id: Some(format!("conversation-deepseek-swapped-{suffix}")),
                    project_id: None,
                    model_id: model_id.to_string(),
                    context_window_indicator_enabled: false,
                    content: "Create a pending approval.".to_string(),
                    attachments: Vec::new(),
                    skills: Vec::new(),
                    title: None,
                    user_message_id: Some(format!("user-deepseek-swapped-{suffix}")),
                    assistant_message_id: Some(format!("assistant-deepseek-swapped-{suffix}")),
                    max_tokens: Some(1_024),
                    temperature: None,
                    prompt_preferences: None,
                    permissions: Default::default(),
                },
                notifications,
            )
            .unwrap();
        wait_for_notification_matching(&mut receiver, |notification| {
            notification["params"]["type"] == "approval_required"
        })
        .await;
        wait_for_notification_matching(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
                && notification["params"]["status"] == "waiting_for_approval"
        })
        .await;
        run_ids.push(turn.run_id);
    }
    model_server.await.unwrap();
    assert_eq!(service.list_pending_actions().len(), 2);
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 0);

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let refs = ["a", "b"].map(|suffix| {
        connection
            .query_row(
                "SELECT continuation_id FROM provider_continuations
                 WHERE conversation_id = ?1 AND state = 'active'",
                [format!("conversation-deepseek-swapped-{suffix}")],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    });
    for (run_id, swapped_ref) in run_ids.iter().zip([&refs[1], &refs[0]]) {
        let persisted_input: String = connection
            .query_row(
                "SELECT agent_input_json FROM agent_pending_actions WHERE run_id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .unwrap();
        let mut persisted_input: Value = serde_json::from_str(&persisted_input).unwrap();
        persisted_input["resumeCheckpoint"]["providerContinuationRefs"][0]["id"] =
            Value::String(swapped_ref.clone());
        connection
            .execute(
                "UPDATE agent_pending_actions SET agent_input_json = ?1 WHERE run_id = ?2",
                rusqlite::params![persisted_input.to_string(), run_id],
            )
            .unwrap();
    }
    drop(connection);
    drop(service);
    drop(storage);

    let reopened_storage = Arc::new(StorageService::open(&database_path).unwrap());
    let reopened_vault = Arc::new(
        mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&reopened_storage),
            credentials,
        )
        .unwrap(),
    );
    let restarted =
        AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
            reopened_storage,
            Some(reopened_vault),
        )
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let action = restarted
        .list_pending_actions()
        .into_iter()
        .find(|action| action.run_id == run_ids[0])
        .expect("first swapped approval must remain pending across restart");
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = restarted
        .approve_action(&run_ids[0], &action.action_id, notifications)
        .unwrap_err();
    assert!(error.contains("provider_continuation"));
    assert_eq!(
        invoker.invocation_count.load(Ordering::SeqCst),
        0,
        "swapped but individually valid refs must fail before invoking the MCP executor"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deepseek_grouped_two_approval_turn_survives_restart_and_pairs_both_provider_results() {
    const FIRST_PROVIDER_CALL_ID: &str = "grouped-first-provider-call";
    const SECOND_PROVIDER_CALL_ID: &str = "grouped-second-provider-call";
    const SECOND_REJECTION: &str = "Reject only the second grouped call.";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let first_request = read_json_request(&mut first).await;
        write_grouped_two_tool_call_stream(
            &mut first,
            FIRST_PROVIDER_CALL_ID,
            SECOND_PROVIDER_CALL_ID,
        )
        .await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let final_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "Grouped approval turn completed.").await;
        drop(second);
        (first_request, final_request)
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("deepseek-grouped-two-approvals.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let conversation_id = "conversation-deepseek-grouped-two-approvals";
    let model_id = "deepseek-grouped-two-approvals-model";
    save_deepseek_approval_provider(
        &storage,
        model_id,
        &format!("http://{address}/v1/chat/completions"),
        mycopilot_core::ReasoningMode::Disabled,
    );
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let vault = Arc::new(
        mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&storage),
            credentials.clone(),
        )
        .unwrap(),
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let service =
        AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
            Arc::clone(&storage),
            Some(vault),
        )
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some(conversation_id.to_string()),
                project_id: None,
                model_id: model_id.to_string(),
                context_window_indicator_enabled: false,
                content: "Create one grouped Assistant turn with two approval calls.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-deepseek-grouped-two-approvals".to_string()),
                assistant_message_id: Some("assistant-deepseek-grouped-two-approvals".to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();
    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "done"
            && notification["params"]["status"] == "waiting_for_approval"
    })
    .await;
    let first_pending = service.list_pending_actions();
    assert_eq!(first_pending.len(), 1);
    let first_action_id = first_pending[0].action_id.clone();

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let (first_ref, encrypted_turn_count): (String, i64) = connection
        .query_row(
            "SELECT continuation_id, COUNT(*) FROM provider_continuations
             WHERE conversation_id = ?1 AND state = 'active'",
            [conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(encrypted_turn_count, 1);
    let first_checkpoint_json: String = connection
        .query_row(
            "SELECT agent_input_json FROM agent_pending_actions
             WHERE run_id = ?1 AND status = 'pending'",
            [&turn.run_id],
            |row| row.get(0),
        )
        .unwrap();
    let first_checkpoint_json: Value = serde_json::from_str(&first_checkpoint_json).unwrap();
    assert_eq!(
        first_checkpoint_json["resumeCheckpoint"]["providerContinuationRefs"][0]["id"],
        first_ref
    );
    assert_eq!(
        first_checkpoint_json["resumeCheckpoint"]["assistantTurnIdentity"]["toolCallIdentities"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    drop(connection);
    drop(notifications);
    drop(receiver);
    drop(service);
    drop(storage);

    let reopened_storage = Arc::new(StorageService::open(&database_path).unwrap());
    let reopened_vault = Arc::new(
        mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&reopened_storage),
            credentials,
        )
        .unwrap(),
    );
    let restarted =
        AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
            Arc::clone(&reopened_storage),
            Some(reopened_vault),
        )
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (restart_notifications, mut restart_receiver) = tokio::sync::mpsc::unbounded_channel();
    restarted
        .approve_action(
            &turn.run_id,
            &first_action_id,
            restart_notifications.clone(),
        )
        .unwrap();

    wait_for_notification_matching(&mut restart_receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    wait_for_notification_matching(&mut restart_receiver, |notification| {
        notification["params"]["type"] == "done"
            && notification["params"]["status"] == "waiting_for_approval"
    })
    .await;
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 1);
    let second_pending = restarted.list_pending_actions();
    assert_eq!(second_pending.len(), 1);
    assert_ne!(second_pending[0].action_id, first_action_id);

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let still_active: (String, i64) = connection
        .query_row(
            "SELECT continuation_id, COUNT(*) FROM provider_continuations
             WHERE conversation_id = ?1 AND state = 'active'",
            [conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(still_active, (first_ref.clone(), 1));
    let second_checkpoint_json: String = connection
        .query_row(
            "SELECT agent_input_json FROM agent_pending_actions
             WHERE action_id = ?1 AND status = 'pending'",
            [pending_action_storage_id(
                &turn.run_id,
                &second_pending[0].action_id,
            )],
            |row| row.get(0),
        )
        .unwrap();
    let second_checkpoint_json: Value = serde_json::from_str(&second_checkpoint_json).unwrap();
    assert_eq!(
        second_checkpoint_json["resumeCheckpoint"]["providerContinuationRefs"][0]["id"],
        first_ref
    );
    assert_eq!(
        second_checkpoint_json["resumeCheckpoint"]["assistantTurnIdentity"]["toolCallIdentities"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    drop(connection);

    restarted
        .reject_action(
            &turn.run_id,
            &second_pending[0].action_id,
            Some(SECOND_REJECTION.to_string()),
            restart_notifications,
        )
        .unwrap();
    let done = wait_for_notification_matching(&mut restart_receiver, |notification| {
        notification["params"]["type"] == "done"
    })
    .await;
    assert_eq!(done["params"]["status"], "completed");

    let (_first_request, final_request) = model_server.await.unwrap();
    let messages = final_request["messages"]
        .as_array()
        .expect("final DeepSeek continuation request must contain messages");
    let grouped_assistants = messages
        .iter()
        .filter(|message| {
            message["role"] == "assistant"
                && message["tool_calls"]
                    .as_array()
                    .is_some_and(|calls| calls.len() == 2)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        grouped_assistants.len(),
        1,
        "the original provider Assistant turn must not be split across restart boundaries"
    );
    let grouped_ids = grouped_assistants[0]["tool_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| call["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        grouped_ids,
        vec![FIRST_PROVIDER_CALL_ID, SECOND_PROVIDER_CALL_ID]
    );
    let provider_results = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .filter_map(|message| {
            message["tool_call_id"]
                .as_str()
                .map(|call_id| (call_id, message["content"].as_str().unwrap_or_default()))
        })
        .filter(|(call_id, _)| matches!(*call_id, FIRST_PROVIDER_CALL_ID | SECOND_PROVIDER_CALL_ID))
        .collect::<Vec<_>>();
    assert_eq!(provider_results.len(), 2);
    assert_eq!(provider_results[0].0, FIRST_PROVIDER_CALL_ID);
    let first_result: Value = serde_json::from_str(provider_results[0].1).unwrap();
    assert_eq!(first_result["type"], "mcp_tool");
    assert_eq!(first_result["status"], "completed");
    assert_eq!(first_result["dispatchCertainty"], "response_received");
    assert_eq!(provider_results[1].0, SECOND_PROVIDER_CALL_ID);
    let second_result: Value = serde_json::from_str(provider_results[1].1).unwrap();
    assert_eq!(second_result["type"], "mcp_tool");
    assert_eq!(second_result["status"], "rejected");
    assert_eq!(second_result["userFeedback"], SECOND_REJECTION);
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_service_approval_cas_runs_once_and_keeps_mcp_values_out_of_durable_state() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "Approval lifecycle complete.").await;
        drop(second);
        (first_request, second_request)
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("mcp-approval-lifecycle.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "mcp-approval-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let _notification_guard = notifications.clone();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-approval-lifecycle".to_string()),
                project_id: None,
                model_id: "mcp-approval-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned MCP approval fixture.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-approval-lifecycle".to_string()),
                assistant_message_id: Some("assistant-mcp-approval-lifecycle".to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();

    let (approval_required, pre_approval_events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "approval_required"
        })
        .await;
    assert!(pre_approval_events.iter().any(|notification| {
        notification["params"]["type"] == "mcp_tool_invocation_state_changed"
            && notification["params"]["invocation"]["state"] == "pending_approval"
    }));
    assert!(pre_approval_events.iter().all(|notification| {
        notification["params"]["type"] != "tool_call"
            || notification["params"]["call"]["tool"] != "mcp__approval_fixture__echo"
    }));
    for notification in pre_approval_events.iter().filter(|notification| {
        matches!(
            notification["params"]["type"].as_str(),
            Some("started" | "tool_set_changed")
        )
    }) {
        let definitions = notification["params"]["toolDefinitions"]
            .as_array()
            .expect("generic Tool-set event has definitions");
        assert!(definitions
            .iter()
            .all(|definition| definition["name"] != "mcp__approval_fixture__echo"));
        let rendered = notification.to_string();
        assert!(!rendered.contains("Repository-owned approval lifecycle fixture"));
        assert!(!rendered.contains("\"value\":{\"type\":\"string\"}"));
    }
    let waiting = wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "done"
            && notification["params"]["status"] == "waiting_for_approval"
    })
    .await;
    assert!(!waiting.to_string().contains(ARGUMENT_CANARY));
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let AgentProposedAction::McpToolCall { approval } = &pending[0].action else {
        panic!("expected typed MCP pending action");
    };
    let arguments_digest = approval.identity.arguments_digest.clone();
    assert_eq!(approval.call.args, json!({}));
    let renderer_approval = approval_required.to_string();
    assert!(!renderer_approval.contains(&arguments_digest));
    assert!(!renderer_approval.contains(ARGUMENT_CANARY));
    let renderer_snapshot = serde_json::to_string(&pending[0]).unwrap();
    assert!(!renderer_snapshot.contains(&arguments_digest));
    assert!(!renderer_snapshot.contains(ARGUMENT_CANARY));
    assert!(!waiting.to_string().contains(&arguments_digest));
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 0);

    let approved = service
        .approve_action(&turn.run_id, &pending[0].action_id, notifications.clone())
        .unwrap();
    assert_eq!(approved.status, "approved");
    assert!(
        approved.tool_result.is_none(),
        "MCP approval response must not create a generic ToolResult Renderer channel"
    );
    assert!(service
        .approve_action(&turn.run_id, &pending[0].action_id, notifications.clone(),)
        .is_err());

    let (completed, post_approval_events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
                && notification["params"]["status"] == "completed"
        })
        .await;
    assert!(post_approval_events.iter().any(|notification| {
        notification["params"]["type"] == "mcp_tool_invocation_state_changed"
            && notification["params"]["invocation"]["state"] == "completed"
    }));
    assert!(post_approval_events.iter().all(|notification| {
        !matches!(
            notification["params"]["type"].as_str(),
            Some("tool_call" | "tool_result")
        )
    }));
    for notification in post_approval_events.iter().filter(|notification| {
        matches!(
            notification["params"]["type"].as_str(),
            Some("started" | "tool_set_changed")
        )
    }) {
        assert!(notification["params"]["toolDefinitions"]
            .as_array()
            .expect("continuation Tool-set event has definitions")
            .iter()
            .all(|definition| definition["name"] != "mcp__approval_fixture__echo"));
    }
    assert!(!completed.to_string().contains(ARGUMENT_CANARY));
    assert!(!completed.to_string().contains(RESULT_CANARY));
    let (first_request, second_request) = model_server.await.unwrap();
    let provider_definition = first_request["tools"]
        .as_array()
        .and_then(|tools| {
            tools.iter().find(|tool| {
                tool["function"]["name"].as_str() == Some("mcp__approval_fixture__echo")
            })
        })
        .expect("provider request retains the MCP Tool definition");
    assert!(provider_definition["function"]["description"]
        .as_str()
        .is_some_and(|description| {
            description.contains("Repository-owned approval lifecycle fixture")
        }));
    assert_eq!(
        provider_definition["function"]["parameters"]["properties"]["value"]["type"],
        "string"
    );
    let encoded = serde_json::to_string(&second_request).unwrap();
    assert!(encoded.contains(RESULT_CANARY));
    assert!(!encoded.contains(ARGUMENT_CANARY));
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 1);

    drop(service);
    drop(storage);
    for entry in std::fs::read_dir(fixture.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(path).unwrap();
            let database_fragment = String::from_utf8_lossy(&bytes);
            assert!(!database_fragment.contains(ARGUMENT_CANARY));
            assert!(!database_fragment.contains(RESULT_CANARY));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_mcp_tool_error_keeps_model_arguments_live_but_not_durable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "Automatic lifecycle complete.").await;
        drop(second);
        (first_request, second_request)
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("mcp-auto-live-arguments.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "mcp-auto-live-arguments-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let mut descriptor = lifecycle_descriptor();
    descriptor.approval_mode = AgentMcpApprovalMode::Auto;
    let invoker = ApprovalLifecycleInvoker::with_behavior(
        descriptor,
        ApprovalInvocationBehavior::ServerToolError,
    );
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let _turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-auto-live-arguments".to_string()),
                project_id: None,
                model_id: "mcp-auto-live-arguments-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned automatic MCP fixture.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-auto-live-arguments".to_string()),
                assistant_message_id: Some("assistant-mcp-auto-live-arguments".to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications,
        )
        .unwrap();

    let (completed, mut observed) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
                && notification["params"]["status"] == "completed"
        })
        .await;
    observed.push(completed);
    assert!(observed
        .iter()
        .all(|notification| notification["params"]["type"] != "approval_required"));
    let renderer_projection = serde_json::to_string(&observed).unwrap();
    assert!(!renderer_projection.contains(ARGUMENT_CANARY));
    assert!(!renderer_projection.contains(RESULT_CANARY));
    assert!(service.list_pending_actions().is_empty());
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 1);

    let (_first_request, second_request) = model_server.await.unwrap();
    assert_eq!(
        provider_mcp_tool_arguments(&second_request, "mcp__approval_fixture__echo"),
        json!({ "value": ARGUMENT_CANARY })
    );
    let second_request_json = serde_json::to_string(&second_request).unwrap();
    assert!(
        second_request_json.contains(RESULT_CANARY),
        "the live model request must retain the authoritative MCP Tool error"
    );

    let trace = storage
        .get_conversation_turn_trace("assistant-mcp-auto-live-arguments")
        .unwrap()
        .expect("automatic MCP run must persist a conversation trace");
    trace.validate().unwrap();
    let trace_json = serde_json::to_string(&trace).unwrap();
    assert!(!trace_json.contains(ARGUMENT_CANARY));
    assert!(!trace_json.contains(RESULT_CANARY));

    let model_log = storage
        .get_conversation_model_context_log("assistant-mcp-auto-live-arguments")
        .unwrap()
        .expect("automatic MCP run must persist a replay-safe model-context log");
    let model_log_json = serde_json::to_string(&model_log).unwrap();
    assert!(!model_log_json.contains(ARGUMENT_CANARY));
    assert!(!model_log_json.contains(RESULT_CANARY));
    assert!(model_log.items.iter().any(|item| {
        item.tool_calls
            .iter()
            .any(|call| call.name == "mcp__approval_fixture__echo" && call.args == json!({}))
    }));
}

async fn run_approved_behavior(behavior: ApprovalInvocationBehavior) -> (Value, Vec<Value>, u64) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "Lifecycle fixture complete.").await;
        drop(second);
        second_request
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open(&fixture.path().join("mcp-lifecycle-behavior.sqlite")).unwrap(),
    );
    save_test_pending_provider(
        &storage,
        "mcp-lifecycle-behavior-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_behavior(lifecycle_descriptor(), behavior);
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-lifecycle-behavior".to_string()),
                project_id: None,
                model_id: "mcp-lifecycle-behavior-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned MCP lifecycle fixture.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-lifecycle-behavior".to_string()),
                assistant_message_id: Some("assistant-mcp-lifecycle-behavior".to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();
    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    service
        .approve_action(&turn.run_id, &pending[0].action_id, notifications)
        .unwrap();

    let lifecycle_events = tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        loop {
            let notification = receiver.recv().await.expect("notification channel closed");
            if notification["params"]["type"] == "mcp_tool_invocation_state_changed" {
                events.push(notification["params"]["invocation"].clone());
            }
            if notification["params"]["type"] == "done"
                && notification["params"]["status"] == "completed"
            {
                return events;
            }
        }
    })
    .await
    .expect("lifecycle behavior scenario timed out");
    let second_request = model_server.await.unwrap();
    let invocation_count = invoker.invocation_count.load(Ordering::SeqCst);
    (second_request, lifecycle_events, invocation_count)
}

struct RejectedApprovalScenario {
    second_request: Value,
    events: Vec<Value>,
    invocation_count: u64,
    call_id: String,
    tool_name: String,
}

async fn run_rejected_approval(message: Option<&str>) -> RejectedApprovalScenario {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "Rejected approval was handled.").await;
        drop(second);
        second_request
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("mcp-rejected-approval.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "mcp-rejected-approval-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let _notification_guard = notifications.clone();
    let assistant_message_id = "assistant-mcp-rejected-approval";
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-rejected-approval".to_string()),
                project_id: None,
                model_id: "mcp-rejected-approval-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned MCP fixture and let the user reject it.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-rejected-approval".to_string()),
                assistant_message_id: Some(assistant_message_id.to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();

    let (_, mut events) = wait_for_notification_matching_with_seen(&mut receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    let (waiting, waiting_events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
                && notification["params"]["status"] == "waiting_for_approval"
        })
        .await;
    events.extend(waiting_events);
    events.push(waiting);

    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let AgentProposedAction::McpToolCall { approval } = &pending[0].action else {
        panic!("expected typed MCP pending action");
    };
    let call_id = approval.call.id.clone();
    let tool_name = approval.call.tool.clone();
    assert!(!call_id.is_empty());
    assert_eq!(tool_name, "mcp__approval_fixture__echo");
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 0);

    let rejected = service
        .reject_action(
            &turn.run_id,
            &pending[0].action_id,
            message.map(ToString::to_string),
            notifications,
        )
        .unwrap();
    assert_eq!(rejected.status, "rejected");
    assert!(
        rejected.tool_result.is_none(),
        "MCP rejection stays on the dedicated lifecycle contract at the Renderer boundary"
    );

    let (done, post_rejection_events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
        })
        .await;
    assert_eq!(
        done["params"]["status"], "completed",
        "MCP rejection must resume and complete the Agent run; preceding events: {post_rejection_events:?}"
    );
    assert_eq!(done["params"]["success"], true);
    events.extend(post_rejection_events);
    events.push(done);

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .expect("completed rejected approval must have a durable conversation trace");
    trace.validate().unwrap();
    assert!(trace.items.iter().any(|item| {
        matches!(
            item,
            ConversationTurnTraceItem::ToolResult {
                call_id: result_call_id,
                tool,
                status: mycopilot_core::ConversationTraceToolResultStatus::Rejected,
                success: false,
                ..
            } if result_call_id == &call_id && tool == &tool_name
        )
    }));
    let durable_rejection = trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolResult {
                call_id: result_call_id,
                observation,
                ..
            } if result_call_id == &call_id => observation.as_object(),
            _ => None,
        })
        .expect("durable rejected ToolResult must be a safe object");
    assert_eq!(
        durable_rejection.get("code").and_then(Value::as_str),
        Some("mcp.approval_rejected")
    );
    assert_eq!(
        durable_rejection.get("retryable").and_then(Value::as_bool),
        Some(false)
    );
    for model_only_field in [
        "userFeedback",
        "decisionBy",
        "message",
        "argumentsValidated",
        "callReasonAccepted",
        "argumentsInHistoryRedacted",
    ] {
        assert!(
            !durable_rejection.contains_key(model_only_field),
            "{model_only_field} must stay out of durable trace state"
        );
    }

    let second_request = model_server.await.unwrap();
    let invocation_count = invoker.invocation_count.load(Ordering::SeqCst);
    RejectedApprovalScenario {
        second_request,
        events,
        invocation_count,
        call_id,
        tool_name,
    }
}

fn assert_rejected_mcp_model_result(
    scenario: &RejectedApprovalScenario,
    expected_message: Option<&str>,
) {
    assert_eq!(scenario.invocation_count, 0);

    let messages = scenario.second_request["messages"]
        .as_array()
        .expect("continuation request must include messages");
    let assistant_tool_call = messages
        .iter()
        .filter(|message| message["role"] == "assistant")
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .find(|call| call["id"].as_str() == Some(scenario.call_id.as_str()))
        .expect("continuation must preserve the rejected assistant Tool Call");
    assert_eq!(
        assistant_tool_call["function"]["name"],
        scenario.tool_name.as_str()
    );
    assert_eq!(
        assistant_tool_call["function"]["arguments"], "{}",
        "MCP arguments remain redacted in model history"
    );
    let tool_message = messages
        .iter()
        .find(|message| {
            message["role"] == "tool"
                && message["tool_call_id"].as_str() == Some(scenario.call_id.as_str())
        })
        .expect("continuation must contain the paired rejected ToolResult");
    let content = tool_message["content"]
        .as_str()
        .expect("MCP rejection ToolResult must be JSON text");
    let result: Value = serde_json::from_str(content).expect("MCP rejection must be valid JSON");
    assert_eq!(result["type"], "mcp_tool");
    assert_eq!(result["status"], "rejected");
    assert_eq!(result["outcome"], "rejected");
    assert_eq!(result["dispatchCertainty"], "definitely_not_dispatched");
    assert_eq!(result["decisionBy"], "user");
    assert_eq!(result["executionAttempted"], false);
    assert_eq!(result["argumentsValidated"], true);
    assert_eq!(result["callReasonAccepted"], true);
    assert_eq!(result["argumentsInHistoryRedacted"], true);
    assert_eq!(result["code"], "mcp.approval_rejected");
    assert_eq!(result["retryable"], false);
    match expected_message {
        Some(expected_message) => {
            assert_eq!(result["userFeedback"], expected_message);
            assert_eq!(
                result["retryPolicy"],
                "follow_user_feedback_without_repeating_same_call"
            );
            assert!(result["message"]
                .as_str()
                .is_some_and(|message| message.contains("do not repeat the same call unchanged")));
        }
        None => {
            assert_eq!(
                result["retryPolicy"],
                "new_explicit_user_instruction_required"
            );
            assert!(result["message"]
                .as_str()
                .is_some_and(|message| message.contains("Do not retry")));
            assert!(
                result.get("userFeedback").is_none() || result["userFeedback"].is_null(),
                "a rejection without feedback must not invent user feedback"
            );
        }
    }

    let rejected_lifecycle = scenario
        .events
        .iter()
        .find(|notification| {
            notification["params"]["type"] == "mcp_tool_invocation_state_changed"
                && notification["params"]["invocation"]["state"] == "rejected"
        })
        .expect("rejection must publish its dedicated MCP lifecycle event");
    assert_eq!(
        rejected_lifecycle["params"]["invocation"]["outcome"],
        "rejected"
    );
    assert_eq!(
        rejected_lifecycle["params"]["invocation"]["dispatchCertainty"],
        "definitely_not_dispatched"
    );
    assert_eq!(
        rejected_lifecycle["params"]["invocation"]["errorCode"],
        "mcp.approval_rejected"
    );

    let rendered = serde_json::to_string(&scenario.events).unwrap();
    assert!(!rendered.contains("mcp.tool_outcome_unknown"));
    assert!(!rendered.contains("conversation_trace_persistence_failed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejecting_mcp_approval_without_reason_returns_a_normal_tool_result_without_dispatch() {
    let scenario = run_rejected_approval(None).await;
    assert_rejected_mcp_model_result(&scenario, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejecting_mcp_approval_with_reason_returns_feedback_without_dispatch() {
    const REJECTION_FEEDBACK: &str = "Use the already available information instead.";

    let scenario = run_rejected_approval(Some(REJECTION_FEEDBACK)).await;
    assert_rejected_mcp_model_result(&scenario, Some(REJECTION_FEEDBACK));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_double_reject_has_one_durable_winner_and_one_model_continuation() {
    const REJECTION_FEEDBACK: &str = "Do not use this external tool.";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (continuation_started_sender, continuation_started_receiver) =
        tokio::sync::oneshot::channel();
    let (release_continuation_sender, release_continuation_receiver) =
        tokio::sync::oneshot::channel();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        continuation_started_sender.send(()).unwrap();
        release_continuation_receiver.await.unwrap();
        write_text_stream(&mut second, "One rejection continuation completed.").await;
        drop(second);

        let duplicate_request = tokio::time::timeout(Duration::from_millis(250), listener.accept())
            .await
            .is_ok();
        (second_request, duplicate_request)
    });

    let fixture = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&fixture.path().join("mcp-double-reject.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "mcp-double-reject-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let _notification_guard = notifications.clone();
    let assistant_message_id = "assistant-mcp-double-reject";
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-double-reject".to_string()),
                project_id: None,
                model_id: "mcp-double-reject-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned MCP fixture and let rejection race.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-double-reject".to_string()),
                assistant_message_id: Some(assistant_message_id.to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();

    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "done"
            && notification["params"]["status"] == "waiting_for_approval"
    })
    .await;
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let AgentProposedAction::McpToolCall { approval } = &pending[0].action else {
        panic!("expected typed MCP pending action");
    };
    let call_id = approval.call.id.clone();
    let tool_name = approval.call.tool.clone();
    let storage_id = pending_action_storage_id(&turn.run_id, &pending[0].action_id);
    inject_manual_action_audit_post_commit_failure(&storage_id, "rejected");
    inject_manual_action_audit_post_commit_failure(&storage_id, "rejected");

    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let first_service = service.clone();
    let first_barrier = Arc::clone(&barrier);
    let first_run_id = turn.run_id.clone();
    let first_action_id = pending[0].action_id.clone();
    let first_notifications = notifications.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_service.reject_action(
            &first_run_id,
            &first_action_id,
            Some(REJECTION_FEEDBACK.to_string()),
            first_notifications,
        )
    });
    let second_service = service.clone();
    let second_barrier = Arc::clone(&barrier);
    let second_run_id = turn.run_id.clone();
    let second_action_id = pending[0].action_id.clone();
    let second_notifications = notifications.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_service.reject_action(
            &second_run_id,
            &second_action_id,
            Some(REJECTION_FEEDBACK.to_string()),
            second_notifications,
        )
    });
    barrier.wait().await;
    let outcomes = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "the durable status CAS must elect exactly one rejection continuation: {outcomes:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .find_map(|outcome| outcome.as_ref().ok())
            .expect("one rejection must win")
            .status,
        "rejected"
    );
    continuation_started_receiver.await.unwrap();
    assert!(
        service.process_runs.has_active_run(&turn.run_id),
        "the losing reject must not unregister the winner's pre-spawn/continuation lease"
    );
    release_continuation_sender.send(()).unwrap();

    let (done, mut events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
                && notification["params"]["status"] == "completed"
        })
        .await;
    assert_eq!(done["params"]["success"], true);
    events.push(done);
    assert_eq!(
        events
            .iter()
            .filter(|notification| {
                notification["params"]["type"] == "mcp_tool_invocation_state_changed"
                    && notification["params"]["invocation"]["state"] == "rejected"
            })
            .count(),
        1,
        "only the CAS winner may publish the rejection lifecycle"
    );

    let (second_request, duplicate_request) = model_server.await.unwrap();
    assert!(
        !duplicate_request,
        "double rejection must never spawn a second model continuation"
    );
    let scenario = RejectedApprovalScenario {
        second_request,
        events,
        invocation_count: invoker.invocation_count.load(Ordering::SeqCst),
        call_id: call_id.clone(),
        tool_name: tool_name.clone(),
    };
    assert_rejected_mcp_model_result(&scenario, Some(REJECTION_FEEDBACK));

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .expect("the single rejection continuation must leave a durable trace");
    trace.validate().unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ConversationTurnTraceItem::ToolResult {
                        call_id: result_call_id,
                        tool,
                        status: mycopilot_core::ConversationTraceToolResultStatus::Rejected,
                        ..
                    } if result_call_id == &call_id && tool == &tool_name
                )
            })
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_approved_mcp_rejection_keeps_feedback_and_resumes_with_normal_tool_result() {
    const REJECTION_FEEDBACK: &str = "Use the user's correction after restart.";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "Recovered rejection was handled.").await;
        drop(second);
        second_request
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("mcp-recovered-approved-reject.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "mcp-recovered-approved-reject-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let initial = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (initial_notifications, mut initial_receiver) = tokio::sync::mpsc::unbounded_channel();
    let _initial_notification_guard = initial_notifications.clone();
    let assistant_message_id = "assistant-mcp-recovered-approved-reject";
    let turn = initial
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-recovered-approved-reject".to_string()),
                project_id: None,
                model_id: "mcp-recovered-approved-reject-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned MCP fixture before restart.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-recovered-approved-reject".to_string()),
                assistant_message_id: Some(assistant_message_id.to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            initial_notifications,
        )
        .unwrap();

    wait_for_notification_matching(&mut initial_receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    wait_for_notification_matching(&mut initial_receiver, |notification| {
        notification["params"]["type"] == "done"
            && notification["params"]["status"] == "waiting_for_approval"
    })
    .await;
    let pending = initial.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let AgentProposedAction::McpToolCall { approval } = &pending[0].action else {
        panic!("expected typed MCP pending action");
    };
    let call_id = approval.call.id.clone();
    let tool_name = approval.call.tool.clone();
    let storage_id = pending_action_storage_id(&turn.run_id, &pending[0].action_id);
    let record = initial
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    initial
        .transition_pending_status(&record, PendingActionStatus::Approved)
        .unwrap();
    drop(initial);

    let restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_mcp_startup_inspector(Arc::new(DurableApprovalStartupInspector))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    assert_eq!(restarted.reconcile_startup_mcp_actions().unwrap(), 0);
    let recovered = restarted.list_pending_actions();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, PendingActionStatus::Approved);

    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let _notification_guard = notifications.clone();
    let output = restarted
        .reject_action(
            &turn.run_id,
            &pending[0].action_id,
            Some(REJECTION_FEEDBACK.to_string()),
            notifications,
        )
        .unwrap();
    assert_eq!(output.status, "rejected");
    assert_eq!(
        output.agent_output.status,
        AgentRunStatus::Running,
        "recovered rejection must resume the model instead of directly failing the run"
    );

    let (done, mut events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
                && notification["params"]["status"] == "completed"
        })
        .await;
    assert_eq!(done["params"]["success"], true);
    events.push(done);
    let second_request = model_server.await.unwrap();
    let scenario = RejectedApprovalScenario {
        second_request,
        events,
        invocation_count: invoker.invocation_count.load(Ordering::SeqCst),
        call_id,
        tool_name,
    };
    assert_rejected_mcp_model_result(&scenario, Some(REJECTION_FEEDBACK));
    assert!(restarted.list_pending_actions().is_empty());
    assert!(
        invoker
            .prepared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty(),
        "the rejection winner must invalidate the prepared payload"
    );

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .expect("recovered rejection must preserve a completed durable trace");
    trace.validate().unwrap();
    let rendered_trace = serde_json::to_string(&trace).unwrap();
    assert!(!rendered_trace.contains(REJECTION_FEEDBACK));
    assert!(!rendered_trace.contains(ARGUMENT_CANARY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejecting_a_second_mcp_call_after_success_preserves_the_exact_trace_prefix() {
    const SECOND_REJECTION_FEEDBACK: &str = "Stop after the first external result.";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_tool_call_stream_with_id(&mut first, "first-mcp-provider-call").await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_tool_call_stream_with_id(&mut second, "second-mcp-provider-call").await;
        drop(second);

        let (mut third, _) = listener.accept().await.unwrap();
        let third_request = read_json_request(&mut third).await;
        write_text_stream(&mut third, "Second approval rejection was handled.").await;
        drop(third);
        (second_request, third_request)
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open(&fixture.path().join("mcp-success-then-reject.sqlite")).unwrap(),
    );
    save_test_pending_provider(
        &storage,
        "mcp-success-then-reject-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let _notification_guard = notifications.clone();
    let assistant_message_id = "assistant-mcp-success-then-reject";
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-success-then-reject".to_string()),
                project_id: None,
                model_id: "mcp-success-then-reject-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned MCP fixture twice.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-success-then-reject".to_string()),
                assistant_message_id: Some(assistant_message_id.to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();

    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "done"
            && notification["params"]["status"] == "waiting_for_approval"
    })
    .await;
    let first_pending = service.list_pending_actions();
    assert_eq!(first_pending.len(), 1);
    service
        .approve_action(
            &turn.run_id,
            &first_pending[0].action_id,
            notifications.clone(),
        )
        .unwrap();

    let (_, mut second_call_events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "approval_required"
        })
        .await;
    let (second_waiting, waiting_events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
                && notification["params"]["status"] == "waiting_for_approval"
        })
        .await;
    second_call_events.extend(waiting_events);
    second_call_events.push(second_waiting);
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 1);

    let second_pending = service.list_pending_actions();
    assert_eq!(second_pending.len(), 1);
    let AgentProposedAction::McpToolCall {
        approval: second_approval,
    } = &second_pending[0].action
    else {
        panic!("expected a second typed MCP pending action");
    };
    let second_call_id = second_approval.call.id.clone();
    let second_tool_name = second_approval.call.tool.clone();
    service
        .reject_action(
            &turn.run_id,
            &second_pending[0].action_id,
            Some(SECOND_REJECTION_FEEDBACK.to_string()),
            notifications,
        )
        .unwrap();

    let (done, terminal_events) =
        wait_for_notification_matching_with_seen(&mut receiver, |notification| {
            notification["params"]["type"] == "done"
        })
        .await;
    assert_eq!(
        done["params"]["status"], "completed",
        "rejecting the second MCP call must preserve the committed first-call prefix: {terminal_events:?}"
    );
    assert_eq!(done["params"]["success"], true);
    second_call_events.extend(terminal_events);
    second_call_events.push(done);

    let (second_request, third_request) = model_server.await.unwrap();
    assert!(serde_json::to_string(&second_request)
        .unwrap()
        .contains(RESULT_CANARY));
    let messages = third_request["messages"]
        .as_array()
        .expect("final continuation request must include messages");
    let rejected_tool_message = messages
        .iter()
        .find(|message| {
            message["role"] == "tool"
                && message["tool_call_id"].as_str() == Some(second_call_id.as_str())
        })
        .expect("final continuation must pair the rejected second MCP Tool Call");
    let rejected_result: Value = serde_json::from_str(
        rejected_tool_message["content"]
            .as_str()
            .expect("rejected MCP result must be JSON text"),
    )
    .unwrap();
    assert_eq!(rejected_result["type"], "mcp_tool");
    assert_eq!(rejected_result["status"], "rejected");
    assert_eq!(rejected_result["outcome"], "rejected");
    assert_eq!(
        rejected_result["dispatchCertainty"],
        "definitely_not_dispatched"
    );
    assert_eq!(rejected_result["userFeedback"], SECOND_REJECTION_FEEDBACK);
    assert!(messages
        .iter()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .any(|call| {
            call["id"].as_str() == Some(second_call_id.as_str())
                && call["function"]["name"].as_str() == Some(second_tool_name.as_str())
        }));

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .expect("completed two-call scenario must have a durable trace");
    trace.validate().unwrap();
    let successful_results = trace
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::ToolResult { success: true, .. }
            )
        })
        .count();
    assert_eq!(successful_results, 1);
    assert!(trace.items.iter().any(|item| {
        matches!(
            item,
            ConversationTurnTraceItem::ToolResult {
                call_id,
                tool,
                status: mycopilot_core::ConversationTraceToolResultStatus::Rejected,
                success: false,
                ..
            } if call_id == &second_call_id && tool == &second_tool_name
        )
    }));
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 1);

    let rendered = serde_json::to_string(&second_call_events).unwrap();
    assert!(rendered.contains("\"state\":\"rejected\""));
    assert!(rendered.contains("\"dispatchCertainty\":\"definitely_not_dispatched\""));
    assert!(!rendered.contains("mcp.tool_outcome_unknown"));
    assert!(!rendered.contains("conversation_trace_persistence_failed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_cas_bare_cancellation_is_outcome_unknown_and_model_sees_certainty() {
    let (second_request, events, invocation_count) =
        run_approved_behavior(ApprovalInvocationBehavior::BareCancellation).await;
    assert_eq!(invocation_count, 1);
    let encoded = serde_json::to_string(&second_request).unwrap();
    assert!(encoded.contains("mcp.tool_outcome_unknown"));
    assert!(encoded.contains("possibly_dispatched"));
    assert!(!encoded.contains(ARGUMENT_CANARY));

    let states = events
        .iter()
        .map(|event| {
            (
                event["state"].as_str().unwrap_or("<missing>"),
                event["dispatchCertainty"].as_str().unwrap_or("<missing>"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        vec![
            ("dispatching", "possibly_dispatched"),
            ("outcome_unknown", "possibly_dispatched"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authoritative_truncation_reaches_the_terminal_lifecycle_event() {
    let (second_request, events, invocation_count) =
        run_approved_behavior(ApprovalInvocationBehavior::TruncatedSuccess).await;
    assert_eq!(invocation_count, 1);
    let tool_content = second_request["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|message| message["role"] == "tool"))
        .and_then(|message| message["content"].as_str())
        .expect("missing live MCP ToolResult in the continuation request");
    assert!(tool_content.contains("\"truncatedAtSource\":true"));
    let terminal = events.last().expect("missing terminal MCP lifecycle event");
    assert_eq!(terminal["state"], "completed");
    assert_eq!(terminal["dispatchCertainty"], "response_received");
    assert_eq!(terminal["outputTruncated"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_is_error_is_a_completed_tool_response_not_outcome_unknown() {
    let (second_request, events, invocation_count) =
        run_approved_behavior(ApprovalInvocationBehavior::ServerToolError).await;
    assert_eq!(invocation_count, 1);
    let tool_content = second_request["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|message| message["role"] == "tool"))
        .and_then(|message| message["content"].as_str())
        .expect("missing live MCP ToolResult in the continuation request");
    assert!(tool_content.contains("\"status\":\"completed\""));
    assert!(tool_content.contains("\"outcome\":\"tool_error\""));
    assert!(tool_content.contains("\"dispatchCertainty\":\"response_received\""));
    assert!(tool_content.contains("\"isError\":true"));
    assert!(tool_content.contains(RESULT_CANARY));
    assert!(!tool_content.contains("mcp.tool_outcome_unknown"));

    let terminal = events.last().expect("missing terminal MCP lifecycle event");
    assert_eq!(terminal["state"], "completed");
    assert_eq!(terminal["outcome"], "tool_error");
    assert_eq!(terminal["dispatchCertainty"], "response_received");
    assert_eq!(terminal["isError"], true);
    assert_eq!(terminal["errorCode"], "mcp.tool_error");
    assert!(!serde_json::to_string(&events)
        .unwrap()
        .contains("mcp.tool_outcome_unknown"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_dispatched_mcp_call_finishes_the_agent_run_without_model_replay() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        tokio::time::timeout(Duration::from_millis(750), listener.accept())
            .await
            .is_err()
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open(&fixture.path().join("mcp-cancel-lifecycle.sqlite")).unwrap(),
    );
    save_test_pending_provider(
        &storage,
        "mcp-cancel-lifecycle-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_behavior(
        lifecycle_descriptor(),
        ApprovalInvocationBehavior::WaitForCancellation,
    );
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-cancel-lifecycle".to_string()),
                project_id: None,
                model_id: "mcp-cancel-lifecycle-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call and then cancel the owned MCP fixture.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-cancel-lifecycle".to_string()),
                assistant_message_id: Some("assistant-mcp-cancel-lifecycle".to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();
    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    service
        .approve_action(&turn.run_id, &pending[0].action_id, notifications)
        .unwrap();
    invoker.wait_until_invocation_started().await;

    assert!(service.cancel_run(&turn.run_id));
    let (done, seen) = wait_for_notification_matching_with_seen(&mut receiver, |notification| {
        notification["params"]["type"] == "done" && notification["params"]["status"] == "cancelled"
    })
    .await;
    assert_eq!(done["params"]["success"], false);
    let terminal_invocations = seen
        .iter()
        .filter(|notification| {
            notification["params"]["type"] == "mcp_tool_invocation_state_changed"
                && notification["params"]["invocation"]["state"] == "outcome_unknown"
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_invocations.len(), 1);
    assert_eq!(
        terminal_invocations[0]["params"]["invocation"]["dispatchCertainty"],
        "possibly_dispatched"
    );
    assert!(
        model_server.await.unwrap(),
        "a cancelled MCP result must not trigger a second model request"
    );
    assert!(!service.cancel_run(&turn.run_id));

    let conversation = storage
        .load_conversation("conversation-mcp-cancel-lifecycle")
        .unwrap()
        .unwrap();
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == "assistant-mcp-cancel-lifecycle")
        .unwrap();
    let run: Value = serde_json::from_str(assistant.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "cancelled");
    assert_eq!(assistant.status.as_deref(), Some("sent"));
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_result_persistence_failure_ends_the_live_ui_and_reconciles_without_replay() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        tokio::time::timeout(Duration::from_millis(750), listener.accept())
            .await
            .is_err()
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("mcp-persistence-failure.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "mcp-persistence-failure-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-persistence-failure".to_string()),
                project_id: None,
                model_id: "mcp-persistence-failure-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned MCP persistence fixture.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-persistence-failure".to_string()),
                assistant_message_id: Some("assistant-mcp-persistence-failure".to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();
    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let storage_id = pending_action_storage_id(&turn.run_id, &pending[0].action_id);
    inject_manual_action_audit_failure(&storage_id, "completed");
    service
        .approve_action(&turn.run_id, &pending[0].action_id, notifications)
        .unwrap();

    let terminal_error = wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "error"
            && notification["params"]["code"] == "mcp_result_persistence_failed"
    })
    .await;
    assert_eq!(terminal_error["params"]["recoverable"], false);
    assert!(!terminal_error.to_string().contains(ARGUMENT_CANARY));
    assert!(!terminal_error.to_string().contains(RESULT_CANARY));
    assert!(model_server.await.unwrap());
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 1);
    let executing_status: String = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(executing_status, "executing");

    drop(service);
    let restarted = AgentService::try_new(Arc::clone(&storage)).unwrap();
    assert!(restarted.list_pending_actions().is_empty());
    let (reconciled_status, reconciled_action): (String, String) =
        rusqlite::Connection::open(database_path)
            .unwrap()
            .query_row(
                "SELECT status, action_json FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(reconciled_status, "failed");
    assert_eq!(reconciled_action, "{}");
}

async fn wait_for_notification_matching(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    wait_for_notification_matching_with_seen(receiver, predicate)
        .await
        .0
}

async fn wait_for_notification_matching_with_seen(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
    predicate: impl Fn(&Value) -> bool,
) -> (Value, Vec<Value>) {
    let mut seen = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.expect("notification channel closed");
            if predicate(&notification) {
                return (notification, seen);
            }
            seen.push(notification);
        }
    })
    .await;
    result.expect("timed out waiting for Agent notification")
}

async fn read_json_request(stream: &mut TcpStream) -> Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "model fixture closed before request completed");
        request.extend_from_slice(&buffer[..read]);
        if body_start.is_none() {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let start = index + 4;
                let headers = String::from_utf8_lossy(&request[..index]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                    })
                    .unwrap();
                body_start = Some(start);
                expected_length = Some(start + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.unwrap()]).unwrap()
}

async fn write_tool_call_stream(stream: &mut TcpStream) {
    write_tool_call_stream_with_reasoning(stream, None).await;
}

async fn write_tool_call_stream_with_reasoning(
    stream: &mut TcpStream,
    reasoning_content: Option<&str>,
) {
    write_tool_call_stream_with_id_and_reasoning(
        stream,
        "mcp-approval-provider-call",
        reasoning_content,
    )
    .await;
}

async fn write_tool_call_stream_with_id(stream: &mut TcpStream, call_id: &str) {
    write_tool_call_stream_with_id_and_reasoning(stream, call_id, None).await;
}

async fn write_tool_call_stream_with_id_and_reasoning(
    stream: &mut TcpStream,
    call_id: &str,
    reasoning_content: Option<&str>,
) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut delta = json!({
        "role": "assistant",
        "content": "",
        "tool_calls": [{
            "index": 0,
            "id": call_id,
            "type": "function",
            "function": {
                "name": "mcp__approval_fixture__echo",
                "arguments": serde_json::to_string(&json!({
                    "value": ARGUMENT_CANARY
                })).unwrap()
            }
        }]
    });
    if let Some(reasoning_content) = reasoning_content {
        delta["reasoning_content"] = Value::String(reasoning_content.to_string());
    }
    let tool_frame = json!({
        "choices": [{
            "delta": delta,
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
    });
    stream
        .write_all(
            format!("data: {tool_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}

fn assert_files_do_not_contain(database_path: &std::path::Path, canary: &[u8]) {
    for path in [
        database_path.to_path_buf(),
        std::path::PathBuf::from(format!("{}-wal", database_path.display())),
        std::path::PathBuf::from(format!("{}-journal", database_path.display())),
    ] {
        if !path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes.windows(canary.len()).any(|window| window == canary),
            "raw provider continuation leaked into {}",
            path.display()
        );
    }
}

async fn write_grouped_two_tool_call_stream(
    stream: &mut TcpStream,
    first_call_id: &str,
    second_call_id: &str,
) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let arguments = serde_json::to_string(&json!({ "value": ARGUMENT_CANARY })).unwrap();
    let tool_frame = json!({
        "choices": [{
            "delta": {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "index": 0,
                        "id": first_call_id,
                        "type": "function",
                        "function": {
                            "name": "mcp__approval_fixture__echo",
                            "arguments": arguments
                        }
                    },
                    {
                        "index": 1,
                        "id": second_call_id,
                        "type": "function",
                        "function": {
                            "name": "mcp__approval_fixture__echo",
                            "arguments": arguments
                        }
                    }
                ]
            },
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
    });
    stream
        .write_all(
            format!("data: {tool_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}

async fn write_text_stream(stream: &mut TcpStream, content: &str) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let content_frame = json!({
        "choices": [{
            "delta": {"role": "assistant", "content": content},
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{"delta": {}, "finish_reason": "stop"}]
    });
    stream
        .write_all(
            format!("data: {content_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}
