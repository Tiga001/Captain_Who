use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mycopilot_core::{
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimeProfile,
    AgentCommandRuntimeResolvedPackage, AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
struct StaticMcpStartupInspector(McpApprovalStartupPayloadState);

impl McpApprovalStartupInspector for StaticMcpStartupInspector {
    fn inspect_startup_payload(
        &self,
        _approval: &mycopilot_core::AgentMcpToolApproval,
    ) -> McpApprovalStartupPayloadState {
        self.0
    }
}

#[derive(Default)]
struct InvalidatingMcpInvoker {
    invalidations: std::sync::atomic::AtomicUsize,
}

impl McpToolInvoker for InvalidatingMcpInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<mycopilot_core::McpAgentToolDescriptor>> {
        Ok(Vec::new())
    }

    fn invalidate_prepared_approval(
        &self,
        _identity: &mycopilot_core::AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        self.invalidations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct RecoverableApprovalRaceInvoker {
    invalidations: std::sync::atomic::AtomicUsize,
    invocations: std::sync::atomic::AtomicUsize,
    storage: Option<Arc<StorageService>>,
}

impl McpToolInvoker for RecoverableApprovalRaceInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<mycopilot_core::McpAgentToolDescriptor>> {
        Ok(Vec::new())
    }

    fn invalidate_prepared_approval(
        &self,
        identity: &mycopilot_core::AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        self.invalidations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(storage) = self.storage.as_ref() {
            storage
                .delete_mcp_approval_envelope(&identity.invocation_id)
                .map_err(mycopilot_core::AgentError::from)?;
        }
        Ok(())
    }

    fn revalidate_approved(
        &self,
        _approval: &mycopilot_core::AgentMcpToolApproval,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn invoke_approved<'a>(
        &'a self,
        _invocation: McpApprovedToolInvocation,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::McpToolInvocationFuture<'a> {
        Box::pin(async move {
            self.invocations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(mycopilot_core::McpToolInvocationResult {
                content: Vec::new(),
                structured_content: None,
                is_error: false,
                truncated_at_source: false,
            })
        })
    }
}

struct AutoJournalObservingInvoker {
    storage: Arc<StorageService>,
    invocations: std::sync::atomic::AtomicUsize,
    saw_executing_before_invoke: std::sync::atomic::AtomicBool,
    return_outcome_unknown: bool,
}

impl AutoJournalObservingInvoker {
    fn new(storage: Arc<StorageService>) -> Arc<Self> {
        Self::with_outcome_unknown(storage, false)
    }

    fn with_outcome_unknown(
        storage: Arc<StorageService>,
        return_outcome_unknown: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            storage,
            invocations: std::sync::atomic::AtomicUsize::new(0),
            saw_executing_before_invoke: std::sync::atomic::AtomicBool::new(false),
            return_outcome_unknown,
        })
    }
}

impl McpToolInvoker for AutoJournalObservingInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<mycopilot_core::McpAgentToolDescriptor>> {
        Ok(Vec::new())
    }

    fn revalidate_approved(
        &self,
        _approval: &mycopilot_core::AgentMcpToolApproval,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn invalidate_prepared_approval(
        &self,
        _identity: &mycopilot_core::AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn invoke_approved<'a>(
        &'a self,
        invocation: McpApprovedToolInvocation,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::McpToolInvocationFuture<'a> {
        Box::pin(async move {
            let storage_id = pending_action_storage_id(
                &invocation.approval.identity.run_id,
                &invocation.approval.identity.action_id,
            );
            let saw_executing = self
                .storage
                .list_recoverable_agent_actions_after_reconciliation()?
                .into_iter()
                .any(|row| row.action_id == storage_id && row.status == "executing");
            self.saw_executing_before_invoke
                .store(saw_executing, std::sync::atomic::Ordering::SeqCst);
            self.invocations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.return_outcome_unknown {
                return Err(AgentError::structured(
                    "mcp.tool_outcome_unknown",
                    "Test transport lost the authoritative response.",
                    json!({
                        "type": "mcp_tool",
                        "code": "outcomeUnknown",
                        "retryable": false,
                        "dispatchCertainty": "possibly_dispatched",
                    }),
                ));
            }
            Ok(mycopilot_core::McpToolInvocationResult {
                content: vec![mycopilot_core::McpToolContentBlock::Text {
                    text: "AUTO_RESULT_CANARY_NOT_DURABLE".to_string(),
                }],
                structured_content: None,
                is_error: false,
                truncated_at_source: false,
            })
        })
    }
}

fn auto_mcp_action(
    run_id: &str,
    action_id: &str,
    invocation_id: &str,
    created_at: i64,
) -> AgentProposedAction {
    let mut action = test_mcp_pending_action(run_id, action_id, invocation_id, created_at);
    let AgentProposedAction::McpToolCall { approval } = &mut action else {
        unreachable!("test helper always creates an MCP action");
    };
    approval.approval_mode = mycopilot_core::AgentMcpApprovalMode::Auto;
    approval.call.approval_status = AgentApprovalStatus::Approved;
    action
}

fn auto_mcp_agent_input(storage: &StorageService, run_id: &str, action_id: &str) -> AgentChatInput {
    let mut input = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(storage, &mut input);
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(storage, run_id, action_id));
    input
}

fn freeze_provider_protocol(
    input: &mut AgentChatInput,
    provider_configuration_revision: String,
    config: mycopilot_core::ProviderProfileConfig,
) {
    let dialect = input
        .api_style
        .map(mycopilot_core::ProviderProtocolDialect::from)
        .unwrap_or_else(|| {
            mycopilot_core::ProviderProtocolDialect::detect_from_api_url(&input.api_url)
        });
    let key = mycopilot_core::ProviderProtocolKey::new(
        dialect,
        &config,
        input.model.clone(),
        Some(provider_configuration_revision.clone()),
    )
    .unwrap();
    input.provider_configuration_revision = Some(provider_configuration_revision);
    input.provider_connection_revision =
        Some(format!("provider-connection-v1:{}", uuid::Uuid::new_v4()));
    input.search_connection_revision =
        Some(format!("search-connection-v1:{}", uuid::Uuid::new_v4()));
    input.provider_profile_config = Some(config);
    input.provider_protocol_key = Some(key);
}

#[tokio::test]
async fn auto_mcp_invokes_only_after_hidden_durable_executing_journal_and_scrubs_terminal_row() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-journal.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let invoker = AutoJournalObservingInvoker::new(Arc::clone(&storage));
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let run_id = "auto-mcp-journal-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let action = auto_mcp_action(
        run_id,
        &action_id,
        &invocation_id,
        mycopilot_core::storage::now_ms(),
    );
    let projection_created_at = mycopilot_core::storage::now_ms();
    storage
        .save_conversation(ChatConversationRecord {
            id: "auto-mcp-conversation".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Backend-owned automatic MCP".to_string(),
            messages: vec![ChatMessageRecord {
                id: "auto-mcp-assistant".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: projection_created_at,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: projection_created_at,
            updated_at: projection_created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let AgentProposedAction::McpToolCall { approval } = action else {
        unreachable!();
    };
    let frozen_approval = approval.clone();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let context = AutoApprovedActionContext::new(
        auto_mcp_agent_input(&storage, run_id, &action_id),
        run_id.to_string(),
        Some("auto-mcp-conversation".to_string()),
        Some("auto-mcp-assistant".to_string()),
        None,
    )
    .with_notifications(notifications);

    let result = service
        .execute_auto_mcp_tool_action(&context, approval, AgentCancellationToken::new())
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(invoker
        .saw_executing_before_invoke
        .load(std::sync::atomic::Ordering::SeqCst));
    assert!(service.list_pending_actions().is_empty());
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());
    while let Ok(notification) = receiver.try_recv() {
        assert_ne!(notification["params"]["type"], "approval_required");
        assert!(
            notification["params"]["status"] != "waiting_for_approval",
            "Auto journal must never become a Renderer approval"
        );
    }

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let terminal: (String, String, String) = rusqlite::Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT status, action_json, agent_input_json
             FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(terminal.0, "completed");
    assert_eq!(terminal.1, "{}");
    assert_eq!(terminal.2, "{}");
    let durable = format!("{}{}", terminal.1, terminal.2);
    assert!(!durable.contains("AUTO_RESULT_CANARY_NOT_DURABLE"));

    let durable_result = mycopilot_core::mcp_tool_result_persistence_projection(&result);
    let trace = mycopilot_core::ConversationTurnTrace {
        schema_version: mycopilot_core::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: "auto-mcp-conversation".to_string(),
        assistant_message_id: "auto-mcp-assistant".to_string(),
        terminal_status: mycopilot_core::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: true,
        items: vec![
            mycopilot_core::ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: frozen_approval.identity.call_id.clone(),
                tool: frozen_approval.identity.provenance.model_tool_name.clone(),
                provenance: AgentToolIdentity::Mcp {
                    provenance: frozen_approval.identity.provenance.clone(),
                },
                operation: json!({}),
                approval_status: AgentApprovalStatus::Approved,
                truncated: true,
            },
            mycopilot_core::ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: frozen_approval.identity.call_id.clone(),
                tool: frozen_approval.identity.provenance.model_tool_name.clone(),
                status: mycopilot_core::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: durable_result.result.clone().unwrap(),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: true,
                archive: Default::default(),
            },
        ],
    };
    let mut in_progress = trace.clone();
    in_progress.terminal_status = mycopilot_core::ConversationTurnTraceTerminalStatus::InProgress;
    storage
        .append_in_progress_conversation_turn_trace(
            &in_progress,
            projection_created_at,
            projection_created_at,
        )
        .unwrap();
    storage
        .replace_conversation_turn_trace(
            &trace,
            projection_created_at,
            projection_created_at.saturating_add(1),
        )
        .unwrap();
    let reloaded = storage
        .load_conversation("auto-mcp-conversation")
        .unwrap()
        .unwrap();
    let run: serde_json::Value = serde_json::from_str(
        reloaded.messages[0]
            .agent_run_json
            .as_deref()
            .expect("terminal MCP projection survives journal scrubbing"),
    )
    .unwrap();
    assert_eq!(run["mcpInvocations"][0]["actionId"], action_id);
    assert_eq!(run["mcpInvocations"][0]["invocationId"], invocation_id);
    assert_eq!(run["mcpInvocations"][0]["state"], "completed");
    assert_eq!(run["mcpInvocations"][0]["outcome"], "succeeded");
    assert_eq!(
        run["mcpInvocations"][0]["dispatchCertainty"],
        "response_received"
    );
    assert_eq!(run["timeline"][0]["type"], "mcp_tool_call");
    let encoded = run.to_string();
    assert!(!encoded.contains("AUTO_RESULT_CANARY_NOT_DURABLE"));
}

#[tokio::test]
async fn live_auto_mcp_outcome_unknown_is_durable_and_never_collapses_to_plain_failure() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-outcome-unknown.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let invoker = AutoJournalObservingInvoker::with_outcome_unknown(Arc::clone(&storage), true);
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let run_id = "auto-mcp-outcome-unknown-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let action = auto_mcp_action(
        run_id,
        &action_id,
        &invocation_id,
        mycopilot_core::storage::now_ms(),
    );
    let AgentProposedAction::McpToolCall { approval } = action else {
        unreachable!();
    };
    let context = AutoApprovedActionContext::new(
        auto_mcp_agent_input(&storage, run_id, &action_id),
        run_id.to_string(),
        None,
        None,
        None,
    );

    let result = service
        .execute_auto_mcp_tool_action(&context, approval, AgentCancellationToken::new())
        .await
        .unwrap();
    assert!(!result.ok);
    assert_eq!(
        result
            .result
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str),
        Some("mcp.tool_outcome_unknown")
    );
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let terminal: (String, String, String) = rusqlite::Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT pending.status, pending.action_json, audit.error
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(terminal.0, "failed");
    assert_eq!(terminal.1, "{}");
    assert_eq!(terminal.2, "mcp.tool_outcome_unknown");

    drop(service);
    let restarted = AgentService::try_new(Arc::clone(&storage)).unwrap();
    assert!(restarted.list_pending_actions().is_empty());
    assert_eq!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a durably settled outcome-unknown invocation must never replay"
    );
}

#[test]
fn startup_auto_mcp_journals_never_replay_and_only_executing_becomes_outcome_unknown() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-restart.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let mut identities = Vec::new();
    for should_claim in [false, true] {
        let run_id = format!("auto-mcp-restart-{should_claim}");
        let action_id = uuid::Uuid::new_v4().to_string();
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let action = auto_mcp_action(&run_id, &action_id, &invocation_id, now);
        let conversation_id = format!("auto-mcp-restart-{should_claim}-conversation");
        let assistant_message_id = format!("auto-mcp-restart-{should_claim}-assistant");
        seed_durable_mcp_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &action,
            now,
        );
        let mut record = service
            .prepare_auto_mcp_action_journal(
                &run_id,
                Some(&conversation_id),
                Some(&assistant_message_id),
                action,
                auto_mcp_agent_input(&storage, &run_id, &action_id),
            )
            .unwrap();
        if should_claim {
            service.claim_auto_mcp_dispatch(&mut record).unwrap();
        }
        assert!(service.list_pending_actions().is_empty());
        identities.push((record.storage_id, should_claim));
    }
    drop(service);

    let invoker = Arc::new(RecoverableApprovalRaceInvoker::default());
    let restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>)
        .with_mcp_startup_inspector(Arc::new(StaticMcpStartupInspector(
            McpApprovalStartupPayloadState::DurableAvailable,
        )));
    assert_eq!(restarted.reconcile_startup_mcp_actions().unwrap(), 2);
    assert!(restarted.list_pending_actions().is_empty());
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());
    assert_eq!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    let connection = rusqlite::Connection::open(database_path).unwrap();
    for (storage_id, was_claimed) in identities {
        let terminal: (String, String, Option<String>) = connection
            .query_row(
                "SELECT pending.status, pending.action_json, audit.error
                 FROM agent_pending_actions pending
                 LEFT JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                 WHERE pending.action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(terminal.0, "failed");
        assert_eq!(terminal.1, "{}");
        assert_eq!(
            terminal.2.as_deref(),
            if was_claimed {
                Some("mcp.tool_outcome_unknown")
            } else {
                Some("mcp.approval_policy_denied")
            }
        );
    }
}

#[test]
fn pending_command_round_trip_keeps_the_host_frozen_runtime_binding() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "secret",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": true },
        "messages": []
    }))
    .unwrap();
    let resolved_packages = vec![AgentCommandRuntimeResolvedPackage {
        name: "pptxgenjs".to_string(),
        version: "4.0.1".to_string(),
    }];
    let profile_revision_material = serde_json::to_vec(&json!({
        "schemaVersion": AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        "profile": AgentCommandRuntimeProfile::Presentations,
        "kind": AgentCommandRuntimeKind::Node,
        "packages": resolved_packages,
    }))
    .unwrap();
    let profile_revision = format!(
        "artifact-runtime-profile-sha256-v1:{:x}",
        Sha256::digest(profile_revision_material)
    );
    let binding = AgentCommandRuntimeBinding {
        schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        profile: AgentCommandRuntimeProfile::Presentations,
        profile_revision,
        provider_id: mycopilot_core::artifact_runtime::ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
        bundle_version: mycopilot_core::artifact_runtime::ARTIFACT_RUNTIME_BUNDLE_VERSION
            .to_string(),
        bundle_revision: "artifact-runtime-bundle-sha256-v1:test".to_string(),
        kind: AgentCommandRuntimeKind::Node,
        runtime_version: "22.23.1".to_string(),
        runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
        resolved_packages,
    };
    let action = AgentProposedAction::Command {
        command: AgentCommandRequest {
            id: "managed-runtime-profile-pending".to_string(),
            command: "node scripts/build.mjs --output deck.pptx".to_string(),
            cwd: None,
            timeout_ms: Some(30_000),
            approval_status: AgentApprovalStatus::Required,
            risk_level: None,
            reason: Some("build an Office artifact".to_string()),
            observe: Some(mycopilot_core::AgentCommandArtifactObservationRequest {
                kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
                expected_outputs: vec!["deck.pptx".to_string()],
                additional_roots: Vec::new(),
            }),
            inputs: Vec::new(),
            runtime_binding: Some(Box::new(binding.clone())),
        },
    };

    let model_call = AgentToolCall {
        id: "managed-runtime-profile-pending".to_string(),
        tool: "run_command".to_string(),
        args: json!({ "command": "node scripts/build.mjs --output deck.pptx" }),
        approval_status: AgentApprovalStatus::Required,
        reason: Some("build an Office artifact".to_string()),
    };
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "managed-runtime-profile-run",
        None,
        &model_call,
        AgentToolIdentity::Builtin {
            tool_name: "run_command".to_string(),
        },
    ));
    let AgentProposedAction::Command { command } = &action else {
        unreachable!();
    };
    mycopilot_core::validate_frozen_agent_command_args(command, &model_call.args).unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    assert!(pending_action_binding_matches(
        "managed-runtime-profile-run",
        None,
        &action,
        &agent_input,
    ));
    assert!(service
        .store_pending_action(
            "managed-runtime-profile-run",
            "managed-runtime-profile-conversation",
            "managed-runtime-profile-assistant",
            action,
            agent_input,
        )
        .unwrap());
    drop(service);

    let reloaded = AgentService::new(Arc::clone(&storage));
    let pending = reloaded
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let stored = pending
        .get(&pending_action_storage_id(
            "managed-runtime-profile-run",
            "managed-runtime-profile-pending",
        ))
        .unwrap();
    assert!(stored.agent_input.model_capabilities.image_input);
    let AgentProposedAction::Command { command } = &stored.snapshot.action else {
        panic!("persisted action must remain a command")
    };
    assert_eq!(command.runtime_binding.as_deref(), Some(&binding));
    let resumed_call = tool_call_for_pending_record(stored).unwrap();
    assert!(resumed_call.args.get("runtimeProfile").is_none());
    assert!(resumed_call.args["runtime"].is_null());
}

#[test]
fn provider_action_id_is_scoped_by_run_and_same_run_reuse_is_strict() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    let original = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "shared-action-id".to_string(),
            tool: "first_tool".to_string(),
            args: json!({ "value": "original" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let AgentProposedAction::ToolCall {
        call: original_call,
    } = &original
    else {
        unreachable!();
    };
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-original",
        None,
        original_call,
        AgentToolIdentity::Unregistered {
            tool_name: original_call.tool.clone(),
        },
    ));
    assert!(service
        .store_pending_action(
            "run-original",
            "conversation-original",
            "assistant-original",
            original.clone(),
            agent_input.clone(),
        )
        .unwrap());
    assert!(!service
        .store_pending_action(
            "run-original",
            "conversation-original",
            "assistant-original",
            original,
            agent_input.clone(),
        )
        .unwrap());

    let cross_run = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "shared-action-id".to_string(),
            tool: "second_tool".to_string(),
            args: json!({ "value": "replacement" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    let AgentProposedAction::ToolCall {
        call: cross_run_call,
    } = &cross_run
    else {
        unreachable!();
    };
    let mut cross_run_input = agent_input.clone();
    cross_run_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-second",
        None,
        cross_run_call,
        AgentToolIdentity::Unregistered {
            tool_name: cross_run_call.tool.clone(),
        },
    ));
    assert!(service
        .store_pending_action(
            "run-second",
            "conversation-second",
            "assistant-second",
            cross_run,
            cross_run_input,
        )
        .unwrap());
    let same_run_collision = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "shared-action-id".to_string(),
            tool: "conflicting_tool".to_string(),
            args: json!({ "value": "conflict" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    let AgentProposedAction::ToolCall {
        call: collision_call,
    } = &same_run_collision
    else {
        unreachable!();
    };
    let mut collision_input = agent_input;
    collision_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-original",
        None,
        collision_call,
        AgentToolIdentity::Unregistered {
            tool_name: collision_call.tool.clone(),
        },
    ));
    let error = service
        .store_pending_action(
            "run-original",
            "conversation-original",
            "assistant-original",
            same_run_collision,
            collision_input,
        )
        .unwrap_err();
    assert!(error.contains("冻结快照冲突"));

    let in_memory = service
        .pending_actions
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner());
    let frozen = in_memory
        .get(&pending_action_storage_id(
            "run-original",
            "shared-action-id",
        ))
        .unwrap();
    assert_eq!(frozen.snapshot.run_id, "run-original");
    assert_eq!(frozen.snapshot.tool_name, "first_tool");
    drop(in_memory);
    let durable = storage.list_pending_agent_actions().unwrap();
    assert_eq!(durable.len(), 2);
    assert!(durable.iter().any(|record| {
        record.run_id == "run-original" && record.action_json.contains("first_tool")
    }));
    assert!(durable.iter().any(|record| {
        record.run_id == "run-second" && record.action_json.contains("second_tool")
    }));
    assert!(durable
        .iter()
        .all(|record| !record.action_json.contains("conflicting_tool")));
}

fn test_mcp_pending_action(
    run_id: &str,
    action_id: &str,
    invocation_id: &str,
    created_at: i64,
) -> AgentProposedAction {
    let server_id = uuid::Uuid::new_v4().to_string();
    let model_tool_name = "mcp__startup_fixture__echo".to_string();
    let raw_tool_name = "echo".to_string();
    let scope = mycopilot_core::AgentMcpServerScope::User;
    let call_id = test_mcp_call_id(action_id);
    AgentProposedAction::McpToolCall {
        approval: Box::new(mycopilot_core::AgentMcpToolApproval {
            identity: mycopilot_core::AgentMcpToolInvocationIdentity {
                action_id: action_id.to_string(),
                invocation_id: invocation_id.to_string(),
                run_id: run_id.to_string(),
                call_id: call_id.clone(),
                provenance: mycopilot_core::AgentMcpToolProvenance {
                    server_id: server_id.clone(),
                    scope: scope.clone(),
                    raw_tool_name: raw_tool_name.clone(),
                    model_tool_name: model_tool_name.clone(),
                    config_epoch: "bf616f04-d3ec-4bd7-825f-731a9f0892f4".to_string(),
                    registry_revision: 11,
                    config_digest: "1".repeat(64),
                    catalog_generation: 1,
                    catalog_digest: "2".repeat(64),
                    catalog_schema_digest: "4".repeat(64),
                    schema_digest: "3".repeat(64),
                    schema_normalizer_version: mycopilot_core::MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
                },
                arguments_digest: mycopilot_core::mcp_tool_arguments_digest(&json!({})).unwrap(),
            },
            call: AgentToolCall {
                id: call_id,
                tool: model_tool_name.clone(),
                args: json!({}),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            },
            summary: mycopilot_core::AgentMcpToolApprovalSummary {
                server_id,
                server_display_name: "Startup fixture".to_string(),
                scope,
                raw_tool_name,
                model_tool_name,
                display_reason: None,
                arguments: mycopilot_core::AgentMcpArgumentSummary {
                    encoded_bytes: 2,
                    top_level_property_count: 0,
                    string_value_count: 0,
                    number_value_count: 0,
                    boolean_value_count: 0,
                    null_value_count: 0,
                    object_value_count: 1,
                    array_value_count: 0,
                    max_depth: 1,
                    truncated: false,
                },
                risk: mycopilot_core::AgentMcpToolRisk::Unknown,
                external: true,
            },
            approval_mode: mycopilot_core::AgentMcpApprovalMode::Prompt,
            payload_persistence: mycopilot_core::AgentMcpApprovalPayloadPersistence::ProcessOnly,
            created_at,
            expires_at: created_at + 60_000,
        }),
    }
}

fn test_mcp_call_id(seed: &str) -> String {
    format!("tc1_{}", URL_SAFE_NO_PAD.encode(Sha256::digest(seed)))
}

fn test_mcp_resume_checkpoint(
    storage: &StorageService,
    run_id: &str,
    action_id: &str,
) -> AgentRunCheckpoint {
    let pending_tool_call_id = test_mcp_call_id(action_id);
    let (_, provider_profile_config, provider_protocol_key) =
        test_frozen_provider_protocol(storage, "test-model", None);
    serde_json::from_value(json!({
        "version": AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        "runId": run_id,
        "contextItems": [{
            "role": "assistant",
            "content": "",
            "images": [],
            "toolCalls": [{
                "id": pending_tool_call_id,
                "name": "mcp__startup_fixture__echo",
                "args": {},
                "providerIdentity": {
                    "providerToolIndex": 0,
                    "providerCallId": pending_tool_call_id,
                    "runtimeCallId": pending_tool_call_id
                }
            }],
            "isError": false,
            "sources": ["model_response"],
            "scope": "conversation",
            "retention": "retained",
            "group": {
                "id": format!("run:{run_id}:tool-exchange:1"),
                "kind": "tool_exchange"
            }
        }],
        "nextModelRequestIndex": 1,
        "queuedToolCalls": [],
        "deferredExternalToolCallCount": 0,
        "suppressedNarration": false,
        "extensionSnapshots": [],
        "toolSet": crate::test_tool_set_checkpoint(),
        "runContext": null,
        "collaborationRunSnapshot": null,
        "modelCapabilities": { "imageInput": false },
        "providerProfileConfig": provider_profile_config,
        "providerProtocolKey": provider_protocol_key,
        "assistantTurnIdentity": crate::test_assistant_turn_identity(&[
            pending_tool_call_id.as_str()
        ]),
        "providerContinuationRefs": [],
        "runWorldState": crate::test_run_world_state(),
        "pendingActionId": action_id,
        "pendingToolCallId": pending_tool_call_id,
        "conversationTraceItems": [],
        "conversationModelContextItems": [],
        "nextConversationTraceSequence": 0,
        "conversationTraceTruncated": false
    }))
    .unwrap()
}

fn test_pending_resume_checkpoint_for_call(
    storage: &StorageService,
    run_id: &str,
    pending_action_id: Option<&str>,
    call: &AgentToolCall,
    provenance: AgentToolIdentity,
) -> AgentRunCheckpoint {
    let seed = pending_action_id.unwrap_or(call.id.as_str());
    let mut checkpoint = test_mcp_resume_checkpoint(storage, run_id, seed);
    let provider_identity = AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: call.id.clone(),
        runtime_call_id: call.id.clone(),
    };
    checkpoint.pending_action_id = pending_action_id.map(ToString::to_string);
    checkpoint.pending_tool_call_id = call.id.clone();
    checkpoint.context_items[0].tool_calls = vec![AgentContextCheckpointToolCall {
        id: call.id.clone(),
        name: call.tool.clone(),
        args: call.args.clone(),
        provider_identity: provider_identity.clone(),
    }];
    checkpoint.assistant_turn_identity = crate::test_assistant_turn_identity(&[call.id.as_str()]);
    checkpoint.conversation_trace_items = vec![ConversationTurnTraceItem::ToolCall {
        sequence: 0,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        operation: call.args.clone(),
        provenance,
        approval_status: call.approval_status,
        truncated: false,
    }];
    checkpoint.conversation_model_context_items = vec![ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![AgentContextCheckpointToolCall {
            id: call.id.clone(),
            name: call.tool.clone(),
            args: call.args.clone(),
            provider_identity,
        }],
        is_error: false,
    }];
    checkpoint.next_conversation_trace_sequence = 1;
    checkpoint
}

fn seed_durable_pending_owner(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call: &AgentToolCall,
    provenance: AgentToolIdentity,
    created_at: i64,
) {
    let agent_run_json = json!({
        "runId": run_id,
        "status": "waiting_for_approval",
        "startedAt": created_at,
        "toolDefinitions": [],
        "toolCalls": [],
        "toolResults": [],
        "approvals": [],
        "diffs": [],
        "fileDrafts": [],
        "webSearchActivities": [],
        "readActivities": [],
        "mcpInvocations": [],
        "timeline": [],
        "messageStreamCheckpoints": {},
        "state": {
            "status": "waiting_for_approval",
            "activeRunId": run_id,
            "lastError": null,
            "updatedAt": created_at
        }
    })
    .to_string();
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Pending action recovery".to_string(),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(agent_run_json),
                ui_state_json: None,
            }],
            created_at,
            updated_at: created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    append_durable_pending_trace(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        call,
        provenance,
        created_at,
    );
}

fn append_durable_pending_trace(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call: &AgentToolCall,
    provenance: AgentToolIdentity,
    created_at: i64,
) {
    let provider_identity = AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: call.id.clone(),
        runtime_call_id: call.id.clone(),
    };
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            provenance,
            operation: call.args.clone(),
            approval_status: call.approval_status,
            truncated: false,
        }],
    };
    let model_context = vec![ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![AgentContextCheckpointToolCall {
            id: call.id.clone(),
            name: call.tool.clone(),
            args: call.args.clone(),
            provider_identity,
        }],
        is_error: false,
    }];
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &model_context,
            created_at,
            created_at,
        )
        .unwrap();
}

fn seed_durable_mcp_pending_owner(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    action: &AgentProposedAction,
    created_at: i64,
) {
    let AgentProposedAction::McpToolCall { approval } = action else {
        panic!("test helper requires an MCP action");
    };
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "MCP pending action recovery".to_string(),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(
                    json!({
                        "runId": run_id,
                        "status": "waiting_for_approval",
                        "startedAt": created_at,
                        "toolDefinitions": [],
                        "toolCalls": [],
                        "toolResults": [],
                        "approvals": [],
                        "diffs": [],
                        "fileDrafts": [],
                        "webSearchActivities": [],
                        "readActivities": [],
                        "mcpInvocations": [{
                            "actionId": approval.identity.action_id,
                            "invocationId": approval.identity.invocation_id,
                            "callId": approval.identity.call_id,
                            "serverId": approval.identity.provenance.server_id,
                            "serverDisplayName": approval.summary.server_display_name,
                            "scope": approval.identity.provenance.scope,
                            "rawToolName": approval.identity.provenance.raw_tool_name,
                            "modelToolName": approval.identity.provenance.model_tool_name,
                            "external": true,
                            "state": "pending_approval",
                            "dispatchCertainty": "definitely_not_dispatched",
                            "outputTruncated": false
                        }],
                        "timeline": [{
                            "id": format!("mcp-invocation-{}", approval.identity.invocation_id),
                            "type": "mcp_tool_call",
                            "invocationId": approval.identity.invocation_id
                        }],
                        "messageStreamCheckpoints": {},
                        "state": {
                            "status": "waiting_for_approval",
                            "activeRunId": run_id,
                            "lastError": null,
                            "updatedAt": created_at
                        }
                    })
                    .to_string(),
                ),
                ui_state_json: None,
            }],
            created_at,
            updated_at: created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    append_durable_pending_trace(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &approval.call,
        AgentToolIdentity::Mcp {
            provenance: approval.identity.provenance.clone(),
        },
        created_at,
    );
}

fn freeze_deepseek_tool_checkpoint(
    input: &mut AgentChatInput,
    provider_continuation_refs: Vec<mycopilot_core::ProviderContinuationRef>,
) {
    let mut profile = mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    profile.reasoning = mycopilot_core::ReasoningPolicy {
        mode: mycopilot_core::ReasoningMode::Disabled,
        effort: mycopilot_core::ReasoningEffort::ProviderDefault,
    };
    let key = mycopilot_core::ProviderProtocolKey::new(
        mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        input.model.clone(),
        input.provider_configuration_revision.clone(),
    )
    .unwrap();
    input.provider_profile_config = Some(profile.clone());
    input.provider_protocol_key = Some(key.clone());
    let checkpoint = input.resume_checkpoint.as_mut().unwrap();
    checkpoint.provider_profile_config = profile;
    checkpoint.provider_protocol_key = key;
    checkpoint.provider_continuation_refs = provider_continuation_refs;
}

fn seed_tampered_provider_continuation(
    database_path: &std::path::Path,
    continuation_ref: &mycopilot_core::ProviderContinuationRef,
    protocol: &mycopilot_core::ProviderProtocolKey,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    runtime_call_id: &str,
) {
    let connection = rusqlite::Connection::open(database_path).unwrap();
    connection
        .execute(
            "INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES (?1, NULL, ?2, 'tampered continuation', 1, 1, NULL, NULL, NULL)",
            rusqlite::params![conversation_id, &protocol.model_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO messages (
                id, conversation_id, role, content, status, agent_run_json,
                ui_state_json, created_at, position
             ) VALUES (?1, ?2, 'assistant', 'visible', 'pending', NULL, NULL, 1, 0)",
            rusqlite::params![assistant_message_id, conversation_id],
        )
        .unwrap();
    let protocol_digest = {
        let digest = Sha256::digest(serde_json::to_vec(protocol).unwrap());
        format!("sha256:{digest:x}")
    };
    connection
        .execute(
            "INSERT INTO provider_continuations (
                continuation_id, schema_version, envelope_version,
                conversation_id, assistant_message_id, run_id, request_index,
                assistant_turn_id, assistant_turn_digest, provider_protocol_digest,
                state, superseded_by, compression, encryption, payload_digest,
                nonce, ciphertext, decoded_bytes, compressed_bytes,
                created_at, updated_at, released_at
             ) VALUES (
                ?1, 1, 1, ?2, ?3, ?4, 0,
                ?5, ?6, ?7, 'active', NULL, 'zstd_binary_v1',
                'chacha20_poly1305_v1', ?8, ?9, ?10, 1, 1, 2, 2, NULL
             )",
            rusqlite::params![
                &continuation_ref.id,
                conversation_id,
                assistant_message_id,
                run_id,
                format!("at1_{}", "a".repeat(64)),
                format!("sha256:{}", "b".repeat(64)),
                protocol_digest,
                format!("sha256:{}", "c".repeat(64)),
                vec![7_u8; 12],
                vec![9_u8; 17],
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_continuation_tool_calls (
                continuation_id, provider_tool_index, runtime_call_id
             ) VALUES (?1, 0, ?2)",
            rusqlite::params![&continuation_ref.id, runtime_call_id],
        )
        .unwrap();
}

#[test]
fn provider_continuation_preflight_blocks_empty_missing_and_tampered_mcp_dispatch() {
    for scenario in ["empty", "missing", "tampered"] {
        let fixture = tempdir().unwrap();
        let database_path = fixture
            .path()
            .join(format!("provider-preflight-{scenario}.sqlite"));
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        save_test_pending_provider(
            &storage,
            "test-model",
            "https://example.test/v1/chat/completions",
            "test-token",
            "disabled",
            "",
        );
        let mut provider_settings = storage.load_model_settings().unwrap().unwrap();
        let mut deepseek_profile = mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
        deepseek_profile.reasoning = mycopilot_core::ReasoningPolicy {
            mode: mycopilot_core::ReasoningMode::Disabled,
            effort: mycopilot_core::ReasoningEffort::ProviderDefault,
        };
        provider_settings.models[0].provider_profile_config = deepseek_profile;
        storage.save_model_settings(provider_settings).unwrap();
        let credentials =
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
        let vault = Arc::new(
            mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
                Arc::clone(&storage),
                credentials.clone(),
            )
            .unwrap(),
        );
        let invoker = Arc::new(RecoverableApprovalRaceInvoker::default());
        let service =
            AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
                Arc::clone(&storage),
                Some(vault),
            )
            .unwrap();
        let run_id = format!("provider-preflight-{scenario}-run");
        let conversation_id = format!("provider-preflight-{scenario}-conversation");
        let assistant_message_id = format!("provider-preflight-{scenario}-assistant");
        let action_id = uuid::Uuid::new_v4().to_string();
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let mut input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "test-token",
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "messages": []
        }))
        .unwrap();
        input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, &run_id, &action_id));
        freeze_test_pending_provider_configuration(&storage, &mut input);
        let refs = if scenario == "empty" {
            Vec::new()
        } else {
            vec![mycopilot_core::ProviderContinuationRef::new()]
        };
        freeze_deepseek_tool_checkpoint(&mut input, refs.clone());
        if scenario == "tampered" {
            let checkpoint = input.resume_checkpoint.as_ref().unwrap();
            seed_tampered_provider_continuation(
                &database_path,
                &refs[0],
                &checkpoint.provider_protocol_key,
                &conversation_id,
                &assistant_message_id,
                &run_id,
                &checkpoint.pending_tool_call_id,
            );
        }
        assert!(service
            .store_pending_action(
                &run_id,
                &conversation_id,
                &assistant_message_id,
                test_mcp_pending_action(
                    &run_id,
                    &action_id,
                    &invocation_id,
                    mycopilot_core::storage::now_ms(),
                ),
                input,
            )
            .unwrap());
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
            .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);

        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let error = restarted
            .approve_action(&run_id, &action_id, notifications)
            .unwrap_err();
        assert!(
            error.contains("provider_continuation"),
            "unexpected preflight error for {scenario}: {error}"
        );
        assert_eq!(
            invoker
                .invocations
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{scenario} continuation state must block dispatch before the executor"
        );
        assert!(restarted.list_pending_actions().is_empty());
    }
}

fn test_mcp_envelope(
    invocation_id: &str,
    action_id: &str,
    created_at: i64,
    expires_at: i64,
) -> mycopilot_core::storage::models::McpApprovalEnvelopeRecord {
    mycopilot_core::storage::models::McpApprovalEnvelopeRecord {
        invocation_id: invocation_id.to_string(),
        action_id: action_id.to_string(),
        envelope_version: 3,
        nonce_base64: "AAAAAAAAAAAAAAAA".to_string(),
        ciphertext_base64: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
        aad_digest: "a".repeat(64),
        created_at,
        expires_at,
    }
}

fn assert_recovered_approved_cancellation() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let run_id = "recovered-approved-cancel-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, run_id, &action_id));
    freeze_test_pending_provider_configuration(&storage, &mut input);
    let conversation_id = format!("conversation-{run_id}");
    let assistant_message_id = format!("assistant-{run_id}");
    let action = test_mcp_pending_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        &conversation_id,
        &assistant_message_id,
        run_id,
        &action,
        now,
    );
    assert!(service
        .store_pending_action(
            run_id,
            &conversation_id,
            &assistant_message_id,
            action,
            input,
        )
        .unwrap());
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &invocation_id,
            &storage_id,
            now,
            now + 60_000,
        ))
        .unwrap();
    drop(service);

    let invoker = Arc::new(InvalidatingMcpInvoker::default());
    let restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_mcp_startup_inspector(Arc::new(StaticMcpStartupInspector(
            McpApprovalStartupPayloadState::DurableAvailable,
        )))
        .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);
    assert_eq!(restarted.reconcile_startup_mcp_actions().unwrap(), 0);
    assert_eq!(restarted.list_pending_actions().len(), 1);

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    assert!(restarted.cancel_action(run_id, &action_id).unwrap());

    assert!(restarted.list_pending_actions().is_empty());
    assert!(restarted
        .approve_action(run_id, &action_id, notifications.clone())
        .is_err());
    assert!(restarted
        .reject_action(run_id, &action_id, None, notifications)
        .is_err());
    assert!(!restarted.cancel_action(run_id, &action_id).unwrap());
    assert_eq!(
        invoker
            .invalidations
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(storage
        .load_mcp_approval_envelope(&invocation_id)
        .unwrap()
        .is_none());

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let terminal: (String, Option<String>, String, String, String) = connection
        .query_row(
            "SELECT pending.status, pending.target_status, pending.action_json,
                    pending.agent_input_json, audit.error
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&storage_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    let (expected_status, expected_error) = ("cancelled", "mcp.approval_cancelled");
    assert_eq!(terminal.0, expected_status);
    assert_eq!(terminal.1.as_deref(), Some(expected_status));
    assert_eq!(terminal.2, "{}");
    assert_eq!(terminal.3, "{}");
    assert_eq!(terminal.4, expected_error);
}

#[test]
fn recovered_approved_mcp_can_be_cancelled_with_a_definitely_not_dispatched_terminal_cas() {
    assert_recovered_approved_cancellation();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_approved_mcp_approve_reject_cancel_race_has_one_durable_winner() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let initial = AgentService::new(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let run_id = "recovered-approved-decision-race";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, run_id, &action_id));
    freeze_test_pending_provider_configuration(&storage, &mut input);
    let action = test_mcp_pending_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        "recovered-approved-race-conversation",
        "recovered-approved-race-assistant",
        run_id,
        &action,
        now,
    );
    assert!(initial
        .store_pending_action(
            run_id,
            "recovered-approved-race-conversation",
            "recovered-approved-race-assistant",
            action,
            input,
        )
        .unwrap());
    let pending = initial
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    initial
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &invocation_id,
            &storage_id,
            now,
            now + 60_000,
        ))
        .unwrap();
    drop(initial);

    // Mirror the production invoker's invalidation boundary: every winning decision deletes the
    // encrypted payload envelope. A counter-only mock made the approve winner leave the fixture's
    // separately seeded envelope behind, so the race test passed or timed out depending on which
    // decision happened to win.
    let invoker = Arc::new(RecoverableApprovalRaceInvoker {
        storage: Some(Arc::clone(&storage)),
        ..RecoverableApprovalRaceInvoker::default()
    });
    let recovered = (0..3)
        .map(|_| {
            let service =
                AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
                    .unwrap()
                    .with_mcp_startup_inspector(Arc::new(StaticMcpStartupInspector(
                        McpApprovalStartupPayloadState::DurableAvailable,
                    )))
                    .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);
            assert_eq!(service.reconcile_startup_mcp_actions().unwrap(), 0);
            service
        })
        .collect::<Vec<_>>();
    let barrier = Arc::new(tokio::sync::Barrier::new(4));

    let approve_service = recovered[0].clone();
    let approve_barrier = Arc::clone(&barrier);
    let approve_action_id = action_id.clone();
    let approve = tokio::spawn(async move {
        approve_barrier.wait().await;
        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        approve_service
            .approve_action(run_id, &approve_action_id, notifications)
            .is_ok()
    });
    let reject_service = recovered[1].clone();
    let reject_barrier = Arc::clone(&barrier);
    let reject_action_id = action_id.clone();
    let reject = tokio::spawn(async move {
        reject_barrier.wait().await;
        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        reject_service
            .reject_action(run_id, &reject_action_id, None, notifications)
            .is_ok()
    });
    let cancel_service = recovered[2].clone();
    let cancel_barrier = Arc::clone(&barrier);
    let cancel_action_id = action_id.clone();
    let cancel = tokio::spawn(async move {
        cancel_barrier.wait().await;
        cancel_service
            .cancel_action(run_id, &cancel_action_id)
            .is_ok_and(|cancelled| cancelled)
    });
    barrier.wait().await;
    let winners = usize::from(approve.await.unwrap())
        + usize::from(reject.await.unwrap())
        + usize::from(cancel.await.unwrap());
    assert_eq!(winners, 1);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let lifecycle = storage
                .list_pending_agent_actions()
                .unwrap()
                .into_iter()
                .find(|record| record.action_id == storage_id)
                .map(|record| (record.status, record.target_status));
            let terminal = lifecycle.as_ref().is_none_or(|(status, target_status)| {
                matches!(
                    status.as_str(),
                    "completed" | "failed" | "rejected" | "cancelled"
                ) || target_status.as_deref().is_some_and(|target_status| {
                    matches!(
                        target_status,
                        "completed" | "failed" | "rejected" | "cancelled"
                    )
                })
            });
            let envelope_released = storage
                .load_mcp_approval_envelope(&invocation_id)
                .unwrap()
                .is_none();
            if terminal && envelope_released {
                break;
            }
            // An approved MCP call commits its terminal target before the following model
            // continuation advances the row itself to that target. The target is the durable
            // recovery boundary this race is proving; do not make the assertion depend on an
            // unrelated Provider continuation completing. Avoid a tight loop of synchronous
            // SQLite reads starving the background Runtime task under full-workspace load.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the winning recovered decision must reach a durable terminal state");
    assert!(storage
        .load_mcp_approval_envelope(&invocation_id)
        .unwrap()
        .is_none());
    assert!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst)
            <= 1
    );
}

#[test]
fn mcp_pending_and_checkpoint_json_freeze_only_the_safe_payload_capability() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let run_id = "mcp-safe-persistence-marker";
    let action_id = uuid::Uuid::new_v4().to_string();
    let action = test_mcp_pending_action(
        run_id,
        &action_id,
        &uuid::Uuid::new_v4().to_string(),
        mycopilot_core::storage::now_ms(),
    );
    let checkpoint = test_mcp_resume_checkpoint(&storage, run_id, &action_id);
    let persisted = serde_json::to_string(&json!({
        "pendingAction": action,
        "checkpoint": checkpoint,
    }))
    .unwrap();

    assert!(persisted.contains("\"payloadPersistence\":\"process_only\""));
    for forbidden in [
        "payloadRef",
        "opaquePayloadRef",
        "ciphertext",
        "credentialRef",
        "secretRef",
    ] {
        assert!(
            !persisted.contains(forbidden),
            "safe pending/checkpoint JSON must not contain {forbidden}"
        );
    }
}

#[derive(Clone)]
struct TestMcpActionSource<'a> {
    server_id: &'a str,
    scope: mycopilot_core::AgentMcpServerScope,
    config_digest: &'a str,
    config_epoch: Option<&'a str>,
    registry_revision: u64,
    catalog_generation: u64,
}

fn store_test_mcp_action_for_source(
    service: &AgentService,
    run_id: &str,
    status: PendingActionStatus,
    source: &TestMcpActionSource<'_>,
    created_at: i64,
) -> (String, String) {
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let mut action = test_mcp_pending_action(run_id, &action_id, &invocation_id, created_at);
    let AgentProposedAction::McpToolCall { approval } = &mut action else {
        unreachable!("test helper always creates an MCP action");
    };
    approval.identity.provenance.server_id = source.server_id.to_string();
    approval.identity.provenance.scope = source.scope.clone();
    approval.identity.provenance.config_digest = source.config_digest.to_string();
    if let Some(config_epoch) = source.config_epoch {
        approval.identity.provenance.config_epoch = config_epoch.to_string();
    }
    approval.identity.provenance.registry_revision = source.registry_revision;
    approval.identity.provenance.catalog_generation = source.catalog_generation;
    approval.summary.server_id = source.server_id.to_string();
    approval.summary.scope = source.scope.clone();

    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &service.storage,
        run_id,
        &action_id,
    ));
    freeze_test_pending_provider_configuration(&service.storage, &mut input);
    let conversation_id = format!("conversation-{run_id}");
    let assistant_message_id = format!("assistant-{run_id}");
    seed_durable_mcp_pending_owner(
        &service.storage,
        &conversation_id,
        &assistant_message_id,
        run_id,
        &action,
        created_at,
    );
    assert!(service
        .store_pending_action(
            run_id,
            &conversation_id,
            &assistant_message_id,
            action,
            input,
        )
        .unwrap());

    let storage_id = pending_action_storage_id(run_id, &action_id);
    if status != PendingActionStatus::Pending {
        let mut pending_actions = service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let record = pending_actions.get_mut(&storage_id).unwrap();
        service
            .persist_pending_status(
                record,
                PendingActionStatus::Pending,
                PendingActionStatus::Approved,
            )
            .unwrap();
        record.snapshot.status = PendingActionStatus::Approved;
        if status == PendingActionStatus::Executing {
            service
                .persist_pending_status(
                    record,
                    PendingActionStatus::Approved,
                    PendingActionStatus::Executing,
                )
                .unwrap();
            record.snapshot.status = PendingActionStatus::Executing;
        }
    }
    (storage_id, invocation_id)
}

#[test]
fn process_only_startup_terminalizes_mcp_pending_actions_and_removes_all_envelopes() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    let now = mycopilot_core::storage::now_ms();
    let live_run_id = "mcp-envelope-live-run";
    let expired_run_id = "mcp-envelope-expired-run";
    let live_action_id = uuid::Uuid::new_v4().to_string();
    let expired_action_id = uuid::Uuid::new_v4().to_string();
    let live_invocation_id = uuid::Uuid::new_v4().to_string();
    let expired_invocation_id = uuid::Uuid::new_v4().to_string();
    let orphan_invocation_id = uuid::Uuid::new_v4().to_string();
    let mut live_agent_input = agent_input.clone();
    live_agent_input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &storage,
        live_run_id,
        &live_action_id,
    ));
    let mut expired_agent_input = agent_input;
    expired_agent_input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &storage,
        expired_run_id,
        &expired_action_id,
    ));
    let live_action =
        test_mcp_pending_action(live_run_id, &live_action_id, &live_invocation_id, now);
    let expired_action = test_mcp_pending_action(
        expired_run_id,
        &expired_action_id,
        &expired_invocation_id,
        now,
    );
    seed_durable_mcp_pending_owner(
        &storage,
        "mcp-envelope-live-conversation",
        "mcp-envelope-live-assistant",
        live_run_id,
        &live_action,
        now,
    );
    seed_durable_mcp_pending_owner(
        &storage,
        "mcp-envelope-expired-conversation",
        "mcp-envelope-expired-assistant",
        expired_run_id,
        &expired_action,
        now,
    );

    assert!(service
        .store_pending_action(
            live_run_id,
            "mcp-envelope-live-conversation",
            "mcp-envelope-live-assistant",
            live_action,
            live_agent_input,
        )
        .unwrap());
    assert!(service
        .store_pending_action(
            expired_run_id,
            "mcp-envelope-expired-conversation",
            "mcp-envelope-expired-assistant",
            expired_action,
            expired_agent_input,
        )
        .unwrap());

    // Headless child Turns do not have a Renderer maintaining `agent_run_json`. Startup
    // terminalization must create the typed MCP card from the validated frozen approval before
    // scrubbing it, rather than requiring a pre-existing Renderer projection.
    {
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        connection
            .execute(
                "UPDATE messages SET agent_run_json = NULL
                 WHERE conversation_id IN (
                    'mcp-envelope-live-conversation',
                    'mcp-envelope-expired-conversation'
                 )",
                [],
            )
            .unwrap();
    }

    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &live_invocation_id,
            &pending_action_storage_id(live_run_id, &live_action_id),
            now,
            now + 60_000,
        ))
        .unwrap();
    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &expired_invocation_id,
            &pending_action_storage_id(expired_run_id, &expired_action_id),
            now - 60_000,
            now - 1,
        ))
        .unwrap();
    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &orphan_invocation_id,
            "missing-mcp-pending-action",
            now,
            now + 60_000,
        ))
        .unwrap();
    drop(service);

    let restarted = AgentService::try_new(Arc::clone(&storage))
        .expect("MCP envelope reconciliation must allow safe Agent startup");
    assert!(restarted.list_pending_actions().is_empty());
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());
    assert!(storage
        .load_mcp_approval_envelope(&live_invocation_id)
        .unwrap()
        .is_none());
    assert!(storage
        .load_mcp_approval_envelope(&expired_invocation_id)
        .unwrap()
        .is_none());
    assert!(storage
        .load_mcp_approval_envelope(&orphan_invocation_id)
        .unwrap()
        .is_none());
    let recovered = storage
        .load_conversation("mcp-envelope-live-conversation")
        .unwrap()
        .unwrap();
    let run: Value =
        serde_json::from_str(recovered.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
    assert_eq!(run["mcpInvocations"][0]["state"], "payload_unavailable");
    assert_eq!(
        run["mcpInvocations"][0]["dispatchCertainty"],
        "definitely_not_dispatched"
    );
    assert_eq!(run["mcpInvocations"][0]["isError"], true);
}

#[test]
fn mcp_startup_terminalization_uses_durable_identity_when_private_action_is_corrupt() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let run_id = "corrupt-mcp-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let action = test_mcp_pending_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        "corrupt-mcp-conversation",
        "corrupt-mcp-assistant",
        run_id,
        &action,
        now,
    );
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, run_id, &action_id));
    freeze_test_pending_provider_configuration(&storage, &mut input);
    assert!(service
        .store_pending_action(
            run_id,
            "corrupt-mcp-conversation",
            "corrupt-mcp-assistant",
            action,
            input,
        )
        .unwrap());
    let row = storage
        .list_pending_agent_actions()
        .unwrap()
        .into_iter()
        .find(|row| row.action_id == storage_id)
        .unwrap();
    let approved_at = row.created_at.saturating_add(1);
    let executing_at = row.created_at.saturating_add(2);
    let terminal_at = row.created_at.saturating_add(3);
    storage
        .transition_pending_agent_action(
            &storage_id,
            "pending",
            "approved",
            &row.agent_input_json,
            approved_at,
        )
        .unwrap();
    storage
        .transition_pending_agent_action(
            &storage_id,
            "approved",
            "executing",
            &row.agent_input_json,
            executing_at,
        )
        .unwrap();

    // Simulate on-disk corruption without weakening the canonical write path. Normal writes are
    // rejected by the schema-level JSON validity constraint.
    let corruption = rusqlite::Connection::open(&database_path).unwrap();
    corruption
        .pragma_update(None, "ignore_check_constraints", 1)
        .unwrap();
    corruption
        .execute(
            "UPDATE agent_pending_actions SET action_json = ?1 WHERE action_id = ?2",
            ["{invalid typed MCP action", storage_id.as_str()],
        )
        .unwrap();
    corruption
        .pragma_update(None, "ignore_check_constraints", 0)
        .unwrap();
    drop(corruption);

    assert!(storage
        .terminalize_mcp_agent_action_on_startup(
            &storage_id,
            "executing",
            McpStartupActionTerminalOutcome::OutcomeUnknown,
            terminal_at,
        )
        .unwrap());
    let (status, action_json): (String, String) = rusqlite::Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT status, action_json FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(action_json, "{}");
    let terminal = storage
        .get_conversation_turn_trace("corrupt-mcp-assistant")
        .unwrap()
        .unwrap();
    assert_eq!(
        terminal.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
}

#[test]
fn typed_startup_keeps_durable_waiting_and_approved_but_never_replays_executing() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let mut identities = Vec::new();
    for status in [
        PendingActionStatus::Pending,
        PendingActionStatus::Approved,
        PendingActionStatus::Executing,
    ] {
        let label = pending_status_label(status);
        let run_id = format!("mcp-typed-startup-{label}-run");
        let action_id = uuid::Uuid::new_v4().to_string();
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let storage_id = pending_action_storage_id(&run_id, &action_id);
        let conversation_id = format!("conversation-{label}");
        let assistant_message_id = format!("assistant-{label}");
        let action = test_mcp_pending_action(&run_id, &action_id, &invocation_id, now);
        seed_durable_mcp_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &action,
            now,
        );
        let mut input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "test-token",
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "messages": []
        }))
        .unwrap();
        input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, &run_id, &action_id));
        freeze_test_pending_provider_configuration(&storage, &mut input);
        assert!(service
            .store_pending_action(
                &run_id,
                &conversation_id,
                &assistant_message_id,
                action,
                input,
            )
            .unwrap());
        if status != PendingActionStatus::Pending {
            let row = storage
                .list_pending_agent_actions()
                .unwrap()
                .into_iter()
                .find(|row| row.action_id == storage_id)
                .unwrap();
            storage
                .transition_pending_agent_action(
                    &storage_id,
                    "pending",
                    "approved",
                    &row.agent_input_json,
                    now + 1,
                )
                .unwrap();
            if status == PendingActionStatus::Executing {
                storage
                    .transition_pending_agent_action(
                        &storage_id,
                        "approved",
                        "executing",
                        &row.agent_input_json,
                        now + 2,
                    )
                    .unwrap();
            }
        }
        identities.push((
            status,
            storage_id,
            conversation_id,
            assistant_message_id,
            invocation_id,
        ));
    }
    drop(service);

    let restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_mcp_startup_inspector(Arc::new(StaticMcpStartupInspector(
            McpApprovalStartupPayloadState::DurableAvailable,
        )));
    assert_eq!(restarted.reconcile_startup_mcp_actions().unwrap(), 1);

    let in_memory = restarted
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for (status, storage_id, _, _, _) in &identities {
        match status {
            PendingActionStatus::Pending | PendingActionStatus::Approved => {
                assert_eq!(in_memory[storage_id].snapshot.status, *status);
            }
            PendingActionStatus::Executing => assert!(!in_memory.contains_key(storage_id)),
            _ => unreachable!(),
        }
    }
    drop(in_memory);

    let visible_waiting = restarted.list_pending_actions();
    assert_eq!(visible_waiting.len(), 2);
    assert!(visible_waiting
        .iter()
        .any(|snapshot| snapshot.status == PendingActionStatus::Pending));
    assert!(visible_waiting
        .iter()
        .any(|snapshot| snapshot.status == PendingActionStatus::Approved));
    assert!(!visible_waiting
        .iter()
        .any(|snapshot| snapshot.status == PendingActionStatus::Executing));

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let executing_id = identities
        .iter()
        .find(|(status, _, _, _, _)| *status == PendingActionStatus::Executing)
        .map(|(_, storage_id, _, _, _)| storage_id)
        .unwrap();
    let terminal: (String, String, String) = connection
        .query_row(
            "SELECT pending.status, pending.action_json, audit.error
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [executing_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(terminal.0, "failed");
    assert_eq!(terminal.1, "{}");
    assert_eq!(terminal.2, "mcp.tool_outcome_unknown");

    let (_, _, conversation_id, _, invocation_id) = identities
        .iter()
        .find(|(status, _, _, _, _)| *status == PendingActionStatus::Executing)
        .unwrap();
    let recovered = storage.load_conversation(conversation_id).unwrap().unwrap();
    let run: Value =
        serde_json::from_str(recovered.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
    let invocations = run["mcpInvocations"].as_array().unwrap();
    assert_eq!(
        invocations
            .iter()
            .filter(|candidate| candidate["invocationId"] == invocation_id.as_str())
            .count(),
        1
    );
    let invocation = invocations
        .iter()
        .find(|candidate| candidate["invocationId"] == invocation_id.as_str())
        .unwrap();
    assert_eq!(invocation["state"], "outcome_unknown");
    assert_eq!(invocation["outcome"], "outcome_unknown");
    assert_eq!(invocation["dispatchCertainty"], "possibly_dispatched");
    assert_eq!(invocation["errorCode"], "mcp.tool_outcome_unknown");
    for forbidden in [
        "rawArguments",
        "rawResult",
        "stderr",
        "payloadRef",
        "ciphertext",
    ] {
        assert!(
            !invocation.as_object().unwrap().contains_key(forbidden),
            "terminal MCP projection must remove {forbidden}"
        );
    }
}

#[test]
fn expired_mcp_approval_is_atomically_failed_before_concurrent_approval_can_dispatch() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let invoker = Arc::new(InvalidatingMcpInvoker::default());
    let invoker_for_service: Arc<dyn McpToolInvoker> = invoker.clone();
    let now = mycopilot_core::storage::now_ms();
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(invoker_for_service)
        .with_mcp_approval_clock(move || now + 60_000);
    let run_id = "mcp-expired-approval-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, run_id, &action_id));
    freeze_test_pending_provider_configuration(&storage, &mut input);
    let action = test_mcp_pending_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        "mcp-expired-conversation",
        "mcp-expired-assistant",
        run_id,
        &action,
        now,
    );
    assert!(service
        .store_pending_action(
            run_id,
            "mcp-expired-conversation",
            "mcp-expired-assistant",
            action,
            input,
        )
        .unwrap());

    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut joins = Vec::new();
    for _ in 0..2 {
        let barrier = Arc::clone(&barrier);
        let service = service.clone();
        let action_id = action_id.clone();
        joins.push(std::thread::spawn(move || {
            let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
            barrier.wait();
            service.approve_action(run_id, &action_id, notifications)
        }));
    }
    barrier.wait();
    let results = joins
        .into_iter()
        .map(|join| join.join().unwrap())
        .collect::<Vec<_>>();
    assert!(results.iter().all(Result::is_err));
    assert_eq!(
        invoker
            .invalidations
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "results={results:?}, rows={:?}",
        storage.list_pending_agent_actions().unwrap()
    );
    assert!(service.list_pending_actions().is_empty());

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let terminal: (String, String, String, String, String) = connection
        .query_row(
            "SELECT pending.status, pending.action_json, pending.agent_input_json,
                    audit.action_json, audit.error
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&storage_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(terminal.0, "failed");
    assert_eq!(terminal.1, "{}");
    assert_eq!(terminal.2, "{}");
    assert_eq!(terminal.3, "{}");
    assert_eq!(terminal.4, "mcp.approval_payload_expired");
}

#[test]
fn server_source_invalidation_atomically_scrubs_predispatch_and_marks_executing_unknown() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let invoker = Arc::new(InvalidatingMcpInvoker::default());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);
    let server_uuid = uuid::Uuid::new_v4();
    let server_id = server_uuid.to_string();
    let other_server_id = uuid::Uuid::new_v4().to_string();
    let selected_digest = "1".repeat(64);
    let other_digest = "9".repeat(64);
    let now = mycopilot_core::storage::now_ms();
    let selected_source = TestMcpActionSource {
        server_id: &server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &selected_digest,
        config_epoch: None,
        registry_revision: 11,
        catalog_generation: 1,
    };
    let filtered_source = TestMcpActionSource {
        server_id: &server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &other_digest,
        config_epoch: None,
        registry_revision: 11,
        catalog_generation: 1,
    };
    let unrelated_source = TestMcpActionSource {
        server_id: &other_server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &selected_digest,
        config_epoch: None,
        registry_revision: 11,
        catalog_generation: 1,
    };
    let newer_same_source_identity = TestMcpActionSource {
        server_id: &server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &selected_digest,
        config_epoch: None,
        registry_revision: 13,
        catalog_generation: 1,
    };

    let mut selected = Vec::new();
    for (suffix, status) in [
        ("pending", PendingActionStatus::Pending),
        ("approved", PendingActionStatus::Approved),
        ("executing", PendingActionStatus::Executing),
    ] {
        selected.push((
            status,
            store_test_mcp_action_for_source(
                &service,
                &format!("server-removal-{suffix}-run"),
                status,
                &selected_source,
                now,
            ),
        ));
    }
    let filtered = store_test_mcp_action_for_source(
        &service,
        "server-removal-filtered-run",
        PendingActionStatus::Pending,
        &filtered_source,
        now,
    );
    let unrelated_mcp = store_test_mcp_action_for_source(
        &service,
        "server-removal-unrelated-mcp-run",
        PendingActionStatus::Pending,
        &unrelated_source,
        now,
    );
    let newer_same_source = store_test_mcp_action_for_source(
        &service,
        "server-removal-newer-same-source-run",
        PendingActionStatus::Pending,
        &newer_same_source_identity,
        now,
    );
    let non_mcp_run_id = "server-removal-non-mcp-run";
    let non_mcp_action_id = "server-removal-non-mcp-action";
    let non_mcp_storage_id = pending_action_storage_id(non_mcp_run_id, non_mcp_action_id);
    let mut non_mcp_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut non_mcp_input);
    let non_mcp_call = AgentToolCall {
        id: non_mcp_action_id.to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    non_mcp_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        non_mcp_run_id,
        None,
        &non_mcp_call,
        AgentToolIdentity::Builtin {
            tool_name: non_mcp_call.tool.clone(),
        },
    ));
    seed_durable_pending_owner(
        &storage,
        "server-removal-non-mcp-conversation",
        "server-removal-non-mcp-assistant",
        non_mcp_run_id,
        &non_mcp_call,
        AgentToolIdentity::Builtin {
            tool_name: non_mcp_call.tool.clone(),
        },
        now,
    );
    assert!(service
        .store_pending_action(
            non_mcp_run_id,
            "server-removal-non-mcp-conversation",
            "server-removal-non-mcp-assistant",
            AgentProposedAction::ToolCall { call: non_mcp_call },
            non_mcp_input,
        )
        .unwrap());

    for (storage_id, invocation_id) in selected
        .iter()
        .map(|(_, identity)| identity)
        .chain(std::iter::once(&filtered))
        .chain(std::iter::once(&unrelated_mcp))
        .chain(std::iter::once(&newer_same_source))
    {
        storage
            .store_mcp_approval_envelope(test_mcp_envelope(
                invocation_id,
                storage_id,
                now,
                now + 60_000,
            ))
            .unwrap();
    }

    let target = McpActionInvalidationTarget::server(McpServerId::from_uuid(server_uuid))
        .with_scope(mycopilot_core::AgentMcpServerScope::User)
        .with_source_config_digest(selected_digest.parse::<McpConfigDigest>().unwrap())
        .prior_to_registry_revision(12);
    let summary = service
        .invalidate_mcp_actions_for_server(&target, McpStartupActionTerminalOutcome::PolicyDenied)
        .unwrap();
    assert_eq!(
        summary,
        McpActionInvalidationSummary {
            terminalized_before_dispatch: 2,
            terminalized_outcome_unknown: 1,
            payload_invalidation_attempts: 3,
            payload_invalidation_failures: 0,
        }
    );
    assert_eq!(
        invoker
            .invalidations
            .load(std::sync::atomic::Ordering::SeqCst),
        3
    );

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    for (status, (storage_id, invocation_id)) in &selected {
        let terminal: (String, String, String, String, String) = connection
            .query_row(
                "SELECT pending.status, pending.action_json, pending.agent_input_json,
                        audit.action_json, audit.error
                 FROM agent_pending_actions pending
                 JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                 WHERE pending.action_id = ?1",
                [storage_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(terminal.0, "failed");
        assert_eq!(terminal.1, "{}");
        assert_eq!(terminal.2, "{}");
        assert_eq!(terminal.3, "{}");
        assert_eq!(
            terminal.4,
            if *status == PendingActionStatus::Executing {
                "mcp.tool_outcome_unknown"
            } else {
                "mcp.approval_policy_denied"
            }
        );
        assert!(storage
            .load_mcp_approval_envelope(invocation_id)
            .unwrap()
            .is_none());
    }
    assert!(storage
        .load_mcp_approval_envelope(&filtered.1)
        .unwrap()
        .is_some());
    assert!(storage
        .load_mcp_approval_envelope(&unrelated_mcp.1)
        .unwrap()
        .is_some());
    assert!(storage
        .load_mcp_approval_envelope(&newer_same_source.1)
        .unwrap()
        .is_some());

    let filtered_summary = service
        .invalidate_mcp_actions_for_server(
            &McpActionInvalidationTarget::server(McpServerId::from_uuid(server_uuid))
                .with_source_config_digest(other_digest.parse::<McpConfigDigest>().unwrap()),
            McpStartupActionTerminalOutcome::PayloadUnavailable,
        )
        .unwrap();
    assert_eq!(filtered_summary.terminalized_before_dispatch, 1);
    assert_eq!(filtered_summary.terminalized_outcome_unknown, 0);
    assert_eq!(filtered_summary.payload_invalidation_attempts, 1);
    assert_eq!(filtered_summary.payload_invalidation_failures, 0);
    let filtered_error: String = connection
        .query_row(
            "SELECT error FROM agent_action_audit WHERE action_id = ?1",
            [&filtered.0],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(filtered_error, "mcp.approval_payload_unavailable");
    assert!(storage
        .load_mcp_approval_envelope(&filtered.1)
        .unwrap()
        .is_none());

    let in_memory = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(in_memory.len(), 3);
    assert!(in_memory.contains_key(&unrelated_mcp.0));
    assert!(in_memory.contains_key(&newer_same_source.0));
    assert!(in_memory.contains_key(&non_mcp_storage_id));
    assert_eq!(
        in_memory[&non_mcp_storage_id].snapshot.status,
        PendingActionStatus::Pending
    );
    drop(in_memory);
    let non_mcp_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&non_mcp_storage_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(non_mcp_status, "pending");
    assert_eq!(
        invoker
            .invalidations
            .load(std::sync::atomic::Ordering::SeqCst),
        4
    );
}

#[test]
fn catalog_generation_invalidation_targets_only_prior_generation_of_same_config_epoch() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let invoker = Arc::new(InvalidatingMcpInvoker::default());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);
    let server_uuid = uuid::Uuid::new_v4();
    let server_id = server_uuid.to_string();
    let config_digest = "7".repeat(64);
    let config_epoch = mycopilot_mcp_client::McpConfigEpoch::new();
    let config_epoch_string = config_epoch.to_string();
    let other_epoch_string = mycopilot_mcp_client::McpConfigEpoch::new().to_string();
    let now = mycopilot_core::storage::now_ms();
    let prior_source = TestMcpActionSource {
        server_id: &server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &config_digest,
        config_epoch: Some(&config_epoch_string),
        registry_revision: 5,
        catalog_generation: 2,
    };
    let current_source = TestMcpActionSource {
        catalog_generation: 3,
        ..prior_source.clone()
    };
    let other_epoch_source = TestMcpActionSource {
        config_epoch: Some(&other_epoch_string),
        catalog_generation: 2,
        ..prior_source.clone()
    };

    let mut prior = Vec::new();
    for (suffix, status) in [
        ("pending", PendingActionStatus::Pending),
        ("approved", PendingActionStatus::Approved),
        ("executing", PendingActionStatus::Executing),
    ] {
        prior.push((
            status,
            store_test_mcp_action_for_source(
                &service,
                &format!("catalog-prior-{suffix}"),
                status,
                &prior_source,
                now,
            ),
        ));
    }
    let current = store_test_mcp_action_for_source(
        &service,
        "catalog-current-pending",
        PendingActionStatus::Pending,
        &current_source,
        now,
    );
    let other_epoch = store_test_mcp_action_for_source(
        &service,
        "catalog-other-epoch-pending",
        PendingActionStatus::Pending,
        &other_epoch_source,
        now,
    );
    for (storage_id, invocation_id) in prior
        .iter()
        .map(|(_, identity)| identity)
        .chain(std::iter::once(&current))
        .chain(std::iter::once(&other_epoch))
    {
        storage
            .store_mcp_approval_envelope(test_mcp_envelope(
                invocation_id,
                storage_id,
                now,
                now + 60_000,
            ))
            .unwrap();
    }

    let summary = service
        .invalidate_mcp_actions_for_server(
            &McpActionInvalidationTarget::server(McpServerId::from_uuid(server_uuid))
                .with_source_config_digest(config_digest.parse::<McpConfigDigest>().unwrap())
                .with_source_config_epoch(config_epoch)
                .prior_to_catalog_generation(3),
            McpStartupActionTerminalOutcome::PolicyDenied,
        )
        .unwrap();
    assert_eq!(summary.terminalized_before_dispatch, 2);
    assert_eq!(summary.terminalized_outcome_unknown, 1);
    assert_eq!(summary.payload_invalidation_attempts, 3);
    assert_eq!(summary.payload_invalidation_failures, 0);

    let connection = rusqlite::Connection::open(database_path).unwrap();
    for (status, (storage_id, invocation_id)) in &prior {
        let terminal: (String, String) = connection
            .query_row(
                "SELECT pending.status, audit.error
                 FROM agent_pending_actions pending
                 JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                 WHERE pending.action_id = ?1",
                [storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(terminal.0, "failed");
        assert_eq!(
            terminal.1,
            if *status == PendingActionStatus::Executing {
                "mcp.tool_outcome_unknown"
            } else {
                "mcp.approval_policy_denied"
            }
        );
        assert!(storage
            .load_mcp_approval_envelope(invocation_id)
            .unwrap()
            .is_none());
    }
    for (storage_id, invocation_id) in [&current, &other_epoch] {
        let status: String = connection
            .query_row(
                "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
                [storage_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending");
        assert!(storage
            .load_mcp_approval_envelope(invocation_id)
            .unwrap()
            .is_some());
    }
}

#[test]
fn pending_store_rejects_missing_checkpoint_or_exact_context_before_persistence() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let mut base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut base_input);
    let now = mycopilot_core::storage::now_ms();

    let missing_action_id = uuid::Uuid::new_v4().to_string();
    let error = service
        .store_pending_action(
            "missing-checkpoint-run",
            "missing-checkpoint-conversation",
            "missing-checkpoint-assistant",
            test_mcp_pending_action(
                "missing-checkpoint-run",
                &missing_action_id,
                &uuid::Uuid::new_v4().to_string(),
                now,
            ),
            base_input.clone(),
        )
        .unwrap_err();
    assert_eq!(
        error,
        "Pending action frozen Tool Call identity is inconsistent."
    );

    let empty_context_action_id = uuid::Uuid::new_v4().to_string();
    let mut empty_context_input = base_input;
    let mut checkpoint =
        test_mcp_resume_checkpoint(&storage, "empty-context-run", &empty_context_action_id);
    checkpoint.context_items.clear();
    empty_context_input.resume_checkpoint = Some(checkpoint);
    let error = service
        .store_pending_action(
            "empty-context-run",
            "empty-context-conversation",
            "empty-context-assistant",
            test_mcp_pending_action(
                "empty-context-run",
                &empty_context_action_id,
                &uuid::Uuid::new_v4().to_string(),
                now,
            ),
            empty_context_input,
        )
        .unwrap_err();
    assert_eq!(
        error,
        "Pending action frozen Tool Call identity is inconsistent."
    );

    assert!(storage.list_pending_agent_actions().unwrap().is_empty());
    assert!(service
        .pending_actions
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner())
        .is_empty());
}

#[test]
fn mcp_pending_identity_mismatch_is_rejected_on_store_and_malformed_resume_is_retired() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let mut base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut base_input);
    let now = mycopilot_core::storage::now_ms();

    let rejected_run_id = "mcp-identity-rejected-run";
    let rejected_action_id = uuid::Uuid::new_v4().to_string();
    let rejected_invocation_id = uuid::Uuid::new_v4().to_string();
    let mut rejected_input = base_input.clone();
    rejected_input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &storage,
        rejected_run_id,
        &uuid::Uuid::new_v4().to_string(),
    ));
    let error = service
        .store_pending_action(
            rejected_run_id,
            "mcp-identity-rejected-conversation",
            "mcp-identity-rejected-assistant",
            test_mcp_pending_action(
                rejected_run_id,
                &rejected_action_id,
                &rejected_invocation_id,
                now,
            ),
            rejected_input,
        )
        .unwrap_err();
    assert_eq!(
        error,
        "Pending action frozen Tool Call identity is inconsistent."
    );
    assert!(storage.list_pending_agent_actions().unwrap().is_empty());

    let persisted_run_id = "mcp-identity-persisted-run";
    let persisted_conversation_id = "mcp-identity-persisted-conversation";
    let persisted_assistant_message_id = "mcp-identity-persisted-assistant";
    let persisted_action_id = uuid::Uuid::new_v4().to_string();
    let persisted_invocation_id = uuid::Uuid::new_v4().to_string();
    let persisted_action = test_mcp_pending_action(
        persisted_run_id,
        &persisted_action_id,
        &persisted_invocation_id,
        now,
    );
    let AgentProposedAction::McpToolCall { approval } = &persisted_action else {
        unreachable!();
    };
    seed_durable_pending_owner(
        &storage,
        persisted_conversation_id,
        persisted_assistant_message_id,
        persisted_run_id,
        &approval.call,
        AgentToolIdentity::Mcp {
            provenance: approval.identity.provenance.clone(),
        },
        now,
    );
    let mut persisted_input = base_input;
    persisted_input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &storage,
        persisted_run_id,
        &persisted_action_id,
    ));
    assert!(service
        .store_pending_action(
            persisted_run_id,
            persisted_conversation_id,
            persisted_assistant_message_id,
            persisted_action,
            persisted_input,
        )
        .unwrap());
    let storage_id = pending_action_storage_id(persisted_run_id, &persisted_action_id);
    let row = storage
        .list_pending_agent_actions()
        .unwrap()
        .into_iter()
        .find(|row| row.action_id == storage_id)
        .unwrap();
    let mut tampered_input = serde_json::from_str::<serde_json::Value>(&row.agent_input_json)
        .expect("versioned projection must be valid JSON");
    tampered_input["resumeCheckpoint"]["pendingActionId"] =
        serde_json::Value::String(uuid::Uuid::new_v4().to_string());
    storage
        .transition_pending_agent_action(
            &storage_id,
            "pending",
            "pending",
            &tampered_input.to_string(),
            now + 1,
        )
        .unwrap();
    drop(service);

    let restarted = AgentService::try_new(Arc::clone(&storage))
        .expect("malformed private resume state must retire from durable ToolCall identity");
    assert!(restarted
        .pending_actions
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner())
        .is_empty());
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let retired: (String, String, String) = connection
        .query_row(
            "SELECT status, action_json, agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        retired,
        ("failed".to_string(), "{}".to_string(), "{}".to_string())
    );
}

#[test]
fn malformed_non_mcp_pending_states_retire_without_restoring_an_executable_action() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let now = mycopilot_core::storage::now_ms();
    let mut storage_ids = Vec::new();

    for status in ["pending", "approved", "executing"] {
        let run_id = format!("malformed-{status}-run");
        let conversation_id = format!("malformed-{status}-conversation");
        let assistant_message_id = format!("malformed-{status}-assistant");
        let call = AgentToolCall {
            id: format!("malformed-{status}-call"),
            tool: "approval_tool".to_string(),
            args: json!({}),
            approval_status: if status == "pending" {
                AgentApprovalStatus::Required
            } else {
                AgentApprovalStatus::Approved
            },
            reason: None,
        };
        seed_durable_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call,
            AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
            now,
        );
        let storage_id = pending_action_storage_id(&run_id, &call.id);
        storage
            .store_pending_agent_action(AgentPendingActionRecord {
                action_id: storage_id.clone(),
                run_id,
                conversation_id: Some(conversation_id),
                assistant_message_id: Some(assistant_message_id),
                action_type: "tool_call".to_string(),
                tool_name: call.tool.clone(),
                tool_call_id: Some(call.id),
                status: status.to_string(),
                target_status: None,
                action_json: json!({ "unsupportedAction": true }).to_string(),
                agent_input_json: json!({ "unsupportedResume": true }).to_string(),
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        storage_ids.push(storage_id);
    }

    let loaded = load_persisted_pending_actions(&storage)
        .expect("malformed private projections must retire from durable ToolCall identity");
    assert!(loaded.is_empty());
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());

    let connection = rusqlite::Connection::open(database_path).unwrap();
    for storage_id in storage_ids {
        let retired: (String, String, String) = connection
            .query_row(
                "SELECT status, action_json, agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            retired,
            ("failed".to_string(), "{}".to_string(), "{}".to_string())
        );
    }
}

#[test]
fn mcp_pending_binding_checks_checkpoint_run_storage_call_and_tool_identity() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let run_id = "mcp-binding-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let action = test_mcp_pending_action(
        run_id,
        &action_id,
        &invocation_id,
        mycopilot_core::storage::now_ms(),
    );
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, run_id, &action_id));
    let model_tool_name = match &action {
        AgentProposedAction::McpToolCall { approval } => {
            approval.identity.provenance.model_tool_name.clone()
        }
        _ => unreachable!(),
    };
    let mut record = AgentPendingActionRecord {
        action_id: pending_action_storage_id(run_id, &action_id),
        run_id: run_id.to_string(),
        conversation_id: None,
        assistant_message_id: None,
        action_type: "mcp_tool_call".to_string(),
        tool_name: model_tool_name,
        tool_call_id: Some(test_mcp_call_id(&action_id)),
        status: "pending".to_string(),
        target_status: None,
        action_json: "{}".to_string(),
        agent_input_json: "{}".to_string(),
        created_at: 1,
        updated_at: 1,
    };
    let AgentProposedAction::McpToolCall { approval } = &action else {
        unreachable!();
    };
    let event_validation = mcp_tool_invocation_event(
        approval,
        McpToolInvocationEventUpdate {
            state: AgentMcpToolInvocationState::PendingApproval,
            dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            outcome: None,
            is_error: None,
            error_code: None,
            duration_ms: None,
            output_truncated: false,
            result_size: None,
            failure_stage: None,
        },
    );
    assert!(
        event_validation.is_ok(),
        "fixture approval must satisfy Core validation: {event_validation:?}"
    );
    assert!(pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));

    record.tool_call_id = Some("tampered-call".to_string());
    assert!(!pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));
    record.tool_call_id = Some(test_mcp_call_id(&action_id));
    record.action_id = "tampered-storage-id".to_string();
    assert!(!pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));
    record.action_id = pending_action_storage_id(run_id, &action_id);
    record.tool_name = "tampered-tool".to_string();
    assert!(!pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));
    record.tool_name = match &action {
        AgentProposedAction::McpToolCall { approval } => {
            approval.identity.provenance.model_tool_name.clone()
        }
        _ => unreachable!(),
    };
    record.run_id = "tampered-run".to_string();
    assert!(!pending_action_binding_matches(
        &record.run_id,
        Some(&record),
        &action,
        &input
    ));
    record.run_id = run_id.to_string();
    input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .pending_tool_call_id = "tampered-call".to_string();
    assert!(!pending_action_binding_matches(
        run_id,
        Some(&record),
        &action,
        &input
    ));
}

#[test]
fn pending_resume_sqlite_row_contains_only_versioned_secret_free_projection() {
    const API_TOKEN_CANARY: &str = "SQLITE_PENDING_API_TOKEN_CANARY_DO_NOT_PERSIST";
    const URL_CANARY: &str = "SQLITE_PENDING_URL_CANARY_DO_NOT_PERSIST";
    const SEARCH_CANARY: &str = "SQLITE_PENDING_SEARCH_CANARY_DO_NOT_PERSIST";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        &format!("https://example.test/v1?authorization={URL_CANARY}"),
        API_TOKEN_CANARY,
        "tavily",
        SEARCH_CANARY,
    );
    let service = AgentService::new(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": format!("https://example.test/v1?authorization={URL_CANARY}"),
        "apiToken": API_TOKEN_CANARY,
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "searchConfig": {
            "mode": "tavily",
            "tavilyApiKey": SEARCH_CANARY
        },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    let frozen_profile = agent_input.provider_profile_config.clone().unwrap();
    let frozen_key = agent_input.provider_protocol_key.clone().unwrap();
    let call = AgentToolCall {
        id: "secret-free-resume-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "secret-free-resume-run",
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    let action = AgentProposedAction::ToolCall { call };
    service
        .store_pending_action(
            "secret-free-resume-run",
            "secret-free-resume-conversation",
            "secret-free-resume-assistant",
            action,
            agent_input,
        )
        .unwrap();
    drop(service);

    let row = storage.list_pending_agent_actions().unwrap().remove(0);
    assert!(row
        .agent_input_json
        .contains("\"resumeInputSchemaVersion\":8"));
    for forbidden_key in ["\"apiUrl\"", "\"apiToken\"", "\"tavilyApiKey\""] {
        assert!(!row.agent_input_json.contains(forbidden_key));
    }
    for canary in [API_TOKEN_CANARY, URL_CANARY, SEARCH_CANARY] {
        assert!(!row.agent_input_json.contains(canary));
    }

    // A broad settings save during the approval pause changes the global revision but not this
    // model's effective wire protocol or search connection. Restart must retain the run's
    // original profile/key instead of recomputing provenance from the broad save revision.
    let mut edited = storage.load_model_settings().unwrap().unwrap();
    edited.models[0].context_window_tokens = Some(256_000);
    edited.models[0].input_price = "1.5".to_string();
    edited.models.push(ModelConfigRecord {
        id: "unrelated-restart-model".to_string(),
        display_name: "Unrelated Restart Model".to_string(),
        api_url_override: Some("https://unrelated-restart.example/v1".to_string()),
        api_token_override: Some("unrelated-restart-token".to_string()),
        supports_image: false,
        context_window_tokens: Some(64_000),
        provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        input_price: "0".to_string(),
        cached_input_price: String::new(),
        output_price: "0".to_string(),
        enabled: true,
    });
    storage.save_model_settings(edited).unwrap();

    let reloaded = AgentService::new(storage);
    let pending = reloaded
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let restored = &pending.values().next().unwrap().agent_input;
    assert_eq!(
        restored.provider_profile_config.as_ref(),
        Some(&frozen_profile)
    );
    assert_eq!(restored.provider_protocol_key.as_ref(), Some(&frozen_key));
    assert_eq!(
        persisted_endpoint_digest(&restored.api_url),
        persisted_endpoint_digest(&format!(
            "https://example.test/v1?authorization={URL_CANARY}"
        ))
    );
    assert!(!restored.api_token.is_empty());
    assert!(restored
        .search_config
        .as_ref()
        .unwrap()
        .tavily_api_key
        .is_some());
}

fn frozen_provider_resume_input(
    storage: &Arc<StorageService>,
    api_url: &str,
    api_token: &str,
    search_mode: &str,
    search_key: Option<&str>,
) -> DecodedPersistedAgentResumeInput {
    let dialect = mycopilot_core::ProviderProtocolDialect::detect_from_api_url(api_url);
    frozen_provider_resume_input_with_profile(
        storage,
        api_url,
        api_token,
        search_mode,
        search_key,
        mycopilot_core::ProviderProfileConfig::generic_for_dialect(dialect),
    )
}

fn frozen_provider_resume_input_with_profile(
    storage: &Arc<StorageService>,
    api_url: &str,
    api_token: &str,
    search_mode: &str,
    search_key: Option<&str>,
    profile: mycopilot_core::ProviderProfileConfig,
) -> DecodedPersistedAgentResumeInput {
    let snapshot = storage.load_model_settings_snapshot().unwrap().unwrap();
    let protocol_revision = snapshot.provider_protocol_revisions["test-model"].clone();
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": api_url,
        "apiToken": api_token,
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "searchConfig": {
            "mode": search_mode,
            "tavilyApiKey": search_key
        },
        "messages": []
    }))
    .unwrap();
    freeze_provider_protocol(&mut input, protocol_revision, profile);
    input.provider_connection_revision =
        Some(snapshot.provider_connection_revisions["test-model"].clone());
    input.search_connection_revision = Some(snapshot.search_connection_revision);
    let call = AgentToolCall {
        id: "provider-resume-call".to_string(),
        tool: "read_file".to_string(),
        args: json!({ "path": "README.md" }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        storage,
        "provider-resume-run",
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    ));
    PersistedAgentResumeInput::decode(
        &PersistedAgentResumeInput::from_agent_input(&input)
            .unwrap()
            .encode(),
    )
    .unwrap()
}

fn valid_resume_collaboration_identity() -> mycopilot_core::AgentCollaborationIdentity {
    mycopilot_core::AgentCollaborationIdentity {
        agent_id: "agent-child-resume".to_string(),
        root_agent_id: "agent-root-resume".to_string(),
        root_conversation_id: "conversation-root-resume".to_string(),
        parent_agent_id: "agent-root-resume".to_string(),
        parent_task_name: "Root".to_string(),
        parent_task_path: "root".to_string(),
        conversation_id: "conversation-child-resume".to_string(),
        task_name: "review".to_string(),
        task_path: "root/review".to_string(),
        source_agent_id: "agent-root-resume".to_string(),
        source_kind: mycopilot_core::AgentMailboxKind::Task,
        source_task_name: "Root".to_string(),
        source_task_path: "root".to_string(),
        source_agent_message_id: "mailbox-task-resume".to_string(),
        entrusted_task: "Review the durable facts.".to_string(),
        template_instructions: None,
    }
}

fn collaboration_resume_input(storage: &StorageService) -> AgentChatInput {
    let snapshot = storage.load_model_settings_snapshot().unwrap().unwrap();
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://resume-identity.example/v1/chat/completions",
        "apiToken": "resume-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "messages": []
    }))
    .unwrap();
    freeze_provider_protocol(
        &mut input,
        snapshot.provider_protocol_revisions["test-model"].clone(),
        mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        ),
    );
    input.provider_connection_revision =
        Some(snapshot.provider_connection_revisions["test-model"].clone());
    input.search_connection_revision = Some(snapshot.search_connection_revision);
    let call = AgentToolCall {
        id: "resume-collaboration-call".to_string(),
        tool: "read_file".to_string(),
        args: json!({ "path": "README.md" }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let context = AgentRunContext {
        conversation_id: Some("conversation-child-resume".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: Some(valid_resume_collaboration_identity()),
    };
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        storage,
        "run-collaboration-resume",
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    );
    checkpoint.run_context = Some(context.clone());
    input.context = Some(context);
    input.resume_checkpoint = Some(checkpoint);
    input
}

#[test]
fn persisted_resume_requires_exact_collaboration_identity_on_both_context_copies() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://resume-identity.example/v1/chat/completions",
        "resume-token",
        "disabled",
        "",
    );
    let input = collaboration_resume_input(&storage);
    let encoded = PersistedAgentResumeInput::from_agent_input(&input)
        .unwrap()
        .encode();
    assert!(PersistedAgentResumeInput::decode(&encoded).is_ok());

    let mut missing: Value = serde_json::from_str(&encoded).unwrap();
    missing["context"]
        .as_object_mut()
        .unwrap()
        .remove("collaborationIdentity");
    assert_eq!(
        PersistedAgentResumeInput::decode(&missing.to_string()).unwrap_err(),
        PersistedAgentResumeInputError::InvalidShape
    );

    let mut forged: Value = serde_json::from_str(&encoded).unwrap();
    forged["context"]["collaborationIdentity"]["entrustedTask"] =
        Value::String("A forged task".to_string());
    assert_eq!(
        PersistedAgentResumeInput::decode(&forged.to_string()).unwrap_err(),
        PersistedAgentResumeInputError::InvalidShape
    );

    let mut old: Value = serde_json::from_str(&encoded).unwrap();
    old["resumeInputSchemaVersion"] = json!(6);
    assert_eq!(
        PersistedAgentResumeInput::decode(&old.to_string()).unwrap_err(),
        PersistedAgentResumeInputError::UnsupportedOrMalformed
    );

    let mut mismatched = input;
    mismatched
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .run_context
        .as_mut()
        .unwrap()
        .collaboration_identity
        .as_mut()
        .unwrap()
        .entrusted_task = "Different but individually valid task".to_string();
    let error = match PersistedAgentResumeInput::from_agent_input(&mismatched) {
        Ok(_) => panic!("mismatched Collaboration identity must be rejected"),
        Err(error) => error,
    };
    assert!(error.contains("disagrees"), "{error}");
}

#[test]
fn pending_resume_rejects_provider_token_replacement_at_the_same_endpoint() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-identity.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "first-fixed-token",
        "disabled",
        "",
    );
    let frozen =
        frozen_provider_resume_input(&storage, endpoint, "first-fixed-token", "disabled", None);

    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "replacement-fixed-token",
        "disabled",
        "",
    );

    let error = restore_agent_input_secrets(&storage, frozen).unwrap_err();
    assert!(error.contains("provider connection no longer matches"));
}

#[test]
fn pending_resume_rejects_tokenless_to_token_presence_drift() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-presence.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "new-fixed-token",
        "disabled",
        "",
    );
    // Simulate a tokenless frozen run bound to this otherwise-current non-secret revision. The
    // bidirectional presence check is independent of the revision CAS and must still fail closed.
    let frozen = frozen_provider_resume_input(&storage, endpoint, "", "disabled", None);

    let error = restore_agent_input_secrets(&storage, frozen).unwrap_err();
    assert!(error.contains("credential presence no longer matches"));
}

#[test]
fn pending_resume_preserves_frozen_protocol_across_unrelated_model_settings_edits() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-model.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-model-token",
        "disabled",
        "",
    );
    let frozen =
        frozen_provider_resume_input(&storage, endpoint, "fixed-model-token", "disabled", None);
    let frozen_profile = frozen.agent_input.provider_profile_config.clone().unwrap();
    let frozen_key = frozen.agent_input.provider_protocol_key.clone().unwrap();
    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].context_window_tokens = Some(256_000);
    settings.models[0].input_price = "1.5".to_string();
    settings.models.push(ModelConfigRecord {
        id: "unrelated-model".to_string(),
        display_name: "Unrelated Model".to_string(),
        api_url_override: Some("https://unrelated-provider.example/v1".to_string()),
        api_token_override: Some("unrelated-token".to_string()),
        supports_image: false,
        context_window_tokens: Some(64_000),
        provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        input_price: "0".to_string(),
        cached_input_price: String::new(),
        output_price: "0".to_string(),
        enabled: true,
    });
    storage.save_model_settings(settings).unwrap();

    let restored = restore_agent_input_secrets(&storage, frozen).unwrap();
    assert_eq!(
        restored.provider_profile_config.as_ref(),
        Some(&frozen_profile)
    );
    assert_eq!(restored.provider_protocol_key.as_ref(), Some(&frozen_key));
    assert_eq!(restored.context_window_tokens, Some(128_000));
    assert_eq!(restored.api_token, "fixed-model-token");
}

#[test]
fn pending_resume_preserves_frozen_generic_profile_when_current_model_selects_deepseek() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-profile-change.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-model-token",
        "disabled",
        "",
    );
    let frozen =
        frozen_provider_resume_input(&storage, endpoint, "fixed-model-token", "disabled", None);

    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].provider_profile_config =
        mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    storage.save_model_settings(settings).unwrap();

    let frozen_profile = frozen.agent_input.provider_profile_config.clone().unwrap();
    let frozen_key = frozen.agent_input.provider_protocol_key.clone().unwrap();
    let restored = restore_agent_input_secrets(&storage, frozen).unwrap();
    assert_eq!(restored.provider_profile_config, Some(frozen_profile));
    assert_eq!(restored.provider_protocol_key, Some(frozen_key));
}

#[test]
fn pending_resume_preserves_frozen_deepseek_profile_after_host_selects_generic() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-profile-freeze.example/v1/chat/completions";
    let token = "fixed-model-token";
    save_test_pending_provider(&storage, "test-model", endpoint, token, "disabled", "");

    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].provider_profile_config =
        mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    storage.save_model_settings(settings).unwrap();
    let deepseek = mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    let frozen = frozen_provider_resume_input_with_profile(
        &storage,
        endpoint,
        token,
        "disabled",
        None,
        deepseek.clone(),
    );
    let frozen_key = frozen.agent_input.provider_protocol_key.clone().unwrap();

    let current = storage.load_model_settings().unwrap().unwrap();
    let mut request = serde_json::to_value(current).unwrap();
    request["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("providerProfileConfig");
    request["models"][0]["previousModelId"] = json!("test-model");
    request["models"][0]["providerProfileUpdate"] = json!({"kind": "select_generic"});
    storage
        .save_model_settings_request(
            serde_json::from_value::<mycopilot_core::storage::models::ModelSettingsSaveRequest>(
                request,
            )
            .unwrap(),
        )
        .unwrap();

    let current = storage.load_model_settings().unwrap().unwrap();
    assert_eq!(
        current.models[0].provider_profile_config.profile.id,
        mycopilot_core::ProviderProfileId::GenericOpenAiChat
    );
    let restored = restore_agent_input_secrets(&storage, frozen).unwrap();
    assert_eq!(restored.provider_profile_config, Some(deepseek));
    assert_eq!(restored.provider_protocol_key, Some(frozen_key));
}

#[test]
fn pending_resume_rejects_provider_endpoint_replacement() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-endpoint.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-model-token",
        "disabled",
        "",
    );
    let frozen =
        frozen_provider_resume_input(&storage, endpoint, "fixed-model-token", "disabled", None);

    save_test_pending_provider(
        &storage,
        "test-model",
        "https://replacement-provider.example/v1",
        "fixed-model-token",
        "disabled",
        "",
    );

    let error = restore_agent_input_secrets(&storage, frozen).unwrap_err();
    assert!(error.contains("provider connection no longer matches"));
}

#[test]
fn pending_resume_rejects_search_credential_replacement() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-search.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-provider-token",
        "tavily",
        "first-fixed-search-key",
    );
    let frozen = frozen_provider_resume_input(
        &storage,
        endpoint,
        "fixed-provider-token",
        "tavily",
        Some("first-fixed-search-key"),
    );
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-provider-token",
        "tavily",
        "replacement-fixed-search-key",
    );

    let error = restore_agent_input_secrets(&storage, frozen).unwrap_err();
    assert!(error.contains("search connection no longer matches"));
}

#[test]
fn startup_reconciliation_failure_prevents_agent_service_startup() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-bad-reconciliation".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "bad reconciliation".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-bad-reconciliation".to_string(),
                role: "assistant".to_string(),
                content: THINKING_PLACEHOLDER.to_string(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some("{}".to_string()),
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();

    // Corrupt the persisted row explicitly so the test continues to cover startup fail-closed
    // behavior while ordinary repository writes remain protected by the canonical CHECK.
    let corruption = rusqlite::Connection::open(&database_path).unwrap();
    corruption
        .pragma_update(None, "ignore_check_constraints", 1)
        .unwrap();
    corruption
        .execute(
            "UPDATE messages SET agent_run_json = ?1 WHERE id = ?2",
            ["not-json", "assistant-bad-reconciliation"],
        )
        .unwrap();
    corruption
        .pragma_update(None, "ignore_check_constraints", 0)
        .unwrap();
    drop(corruption);
    let call = AgentToolCall {
        id: "bad-reconciliation-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::ToolCall { call: call.clone() };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "bad-reconciliation-run",
        Some("bad-reconciliation-call"),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    ));
    append_durable_pending_trace(
        &storage,
        "conversation-bad-reconciliation",
        "assistant-bad-reconciliation",
        "bad-reconciliation-run",
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
        1,
    );
    storage
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: pending_action_storage_id(
                "bad-reconciliation-run",
                "bad-reconciliation-call",
            ),
            run_id: "bad-reconciliation-run".to_string(),
            conversation_id: Some("conversation-bad-reconciliation".to_string()),
            assistant_message_id: Some("assistant-bad-reconciliation".to_string()),
            action_type: "tool_call".to_string(),
            tool_name: "approval_tool".to_string(),
            tool_call_id: Some("bad-reconciliation-call".to_string()),
            status: "approved".to_string(),
            target_status: None,
            action_json: serialize_json(&action),
            // Keep this row on the supported Host projection so the test exercises the generic
            // interrupted-action reconciliation failure rather than legacy-format retirement.
            agent_input_json: PersistedAgentResumeInput::from_agent_input(&agent_input)
                .unwrap()
                .encode(),
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();

    let error = match AgentService::try_new(storage) {
        Ok(_) => panic!("reconciliation failure must prevent startup"),
        Err(error) => error,
    };
    assert!(error.contains("failed to reconcile interrupted pending actions"));
    assert!(error.contains("无法解析中断操作"));
}

#[test]
fn agent_service_startup_retires_an_orphaned_cancelled_conversation_trace() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-orphaned-trace".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "orphaned trace".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-orphaned-trace".to_string(),
                role: "assistant".to_string(),
                content: "partial response".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(
                    json!({
                        "runId": "run-orphaned-trace",
                        "status": "cancelled",
                        "completedAt": 20,
                        "state": {
                            "status": "running",
                            "activeRunId": null,
                            "updatedAt": 10
                        }
                    })
                    .to_string(),
                ),
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-orphaned-trace".to_string(),
        conversation_id: "conversation-orphaned-trace".to_string(),
        assistant_message_id: "assistant-orphaned-trace".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: "Reading the image.".to_string(),
            truncated: false,
        }],
    };
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &[ConversationModelContextItem {
                sequence: 0,
                ordinal: 0,
                role: "assistant".to_string(),
                content: "Reading the image.".to_string(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            }],
            10,
            15,
        )
        .unwrap();

    let _service = AgentService::new(storage.clone());

    let repaired = storage
        .get_conversation_turn_trace("assistant-orphaned-trace")
        .unwrap()
        .unwrap();
    assert_eq!(
        repaired.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(repaired.items, trace.items);
    let conversation = storage
        .load_conversation("conversation-orphaned-trace")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.updated_at, 2);
    let run: Value =
        serde_json::from_str(conversation.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "cancelled");
    assert_eq!(run["state"]["status"], "cancelled");
}

#[test]
fn terminal_pending_action_persistence_redacts_run_scoped_skill_bodies() {
    const MARKER: &str = "PENDING_SKILL_INSTRUCTION_BODY_MUST_NOT_SURVIVE";
    const CATALOG_MARKER: &str = "PENDING_SKILL_CATALOG_BODY_MUST_NOT_SURVIVE";
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    let provider_configuration_revision = format!("provider-protocol-v1:{}", uuid::Uuid::new_v4());
    let provider_profile_config = crate::test_provider_profile_config();
    let provider_protocol_key = mycopilot_core::ProviderProtocolKey::new(
        mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile_config,
        "test-model",
        Some(provider_configuration_revision.clone()),
    )
    .unwrap();
    agent_input.provider_configuration_revision = Some(provider_configuration_revision);
    agent_input.provider_connection_revision =
        Some(format!("provider-connection-v1:{}", uuid::Uuid::new_v4()));
    agent_input.search_connection_revision =
        Some(format!("search-connection-v1:{}", uuid::Uuid::new_v4()));
    agent_input.provider_profile_config = Some(provider_profile_config.clone());
    agent_input.provider_protocol_key = Some(provider_protocol_key.clone());
    agent_input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-revision".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application:test-bundled-skill".to_string(),
            name: "test-bundled-skill".to_string(),
            revision: "package-revision".to_string(),
            source: "bundled:application".to_string(),
            instructions: MARKER.to_string(),
            source_bytes: u64::try_from(MARKER.len()).unwrap(),
            resources: None,
        }],
    });
    agent_input.skill_discovery = Some(mycopilot_core::skills::AgentSkillDiscoverySnapshot {
        schema_version: mycopilot_core::skills::AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
        catalog_revision: "skill-enabled-catalog-sha256-v1:redaction".to_string(),
        prompt_token_budget: mycopilot_core::skills::DEFAULT_SKILL_DISCOVERY_PROMPT_TOKENS,
        skills: vec![mycopilot_core::skills::AgentDiscoverableSkill {
            activation_ref: mycopilot_core::skills::derive_skill_activation_ref(
                "skill-enabled-catalog-sha256-v1:redaction",
                "bundled:application:test-bundled-skill",
                "package-revision",
            ),
            id: "bundled:application:test-bundled-skill".to_string(),
            revision: "package-revision".to_string(),
            name: "test-bundled-skill".to_string(),
            description: CATALOG_MARKER.to_string(),
            source_kind: "bundled".to_string(),
        }],
        max_activated_skills: 8,
        max_total_source_bytes: 512 * 1024,
    });
    let checkpoint_discovery = agent_input.skill_discovery.clone().unwrap();
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-skill-redaction".to_string(),
        pending_action_id: None,
        context_items: vec![
            mycopilot_core::AgentContextCheckpointItem {
                role: "user".to_string(),
                content: format!("<backend_activated_skill>{MARKER}</backend_activated_skill>"),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["skill_instructions".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                group: None,
                origin: Some(mycopilot_core::AgentContextCheckpointOrigin {
                    kind: "skill".to_string(),
                    id: "bundled:application:test-bundled-skill".to_string(),
                }),
            },
            mycopilot_core::AgentContextCheckpointItem {
                role: "user".to_string(),
                content: format!(
                    "<backend_available_skills>{CATALOG_MARKER}</backend_available_skills>"
                ),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["skill_catalog".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                group: None,
                origin: None,
            },
            mycopilot_core::AgentContextCheckpointItem {
                role: "system".to_string(),
                content: "NON_SKILL_CHECKPOINT_CONTENT".to_string(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["runtime_guard".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                group: None,
                origin: None,
            },
        ],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: vec![mycopilot_core::AgentExtensionSnapshot {
            extension_id: "skills".to_string(),
            version: 3,
            state: json!({
                "discovery": checkpoint_discovery,
                "skills": [{
                    "id": "bundled:application:test-bundled-skill",
                    "name": "test-bundled-skill",
                    "revision": "package-revision",
                    "source": "bundled:application",
                    "sourceBytes": MARKER.len(),
                    "hasResources": false,
                    "resourceKinds": [],
                    "activatedBy": "user"
                }]
            }),
        }],
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config,
        provider_protocol_key,
        assistant_turn_identity: crate::test_assistant_turn_identity(&["action-skill-redaction"]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: "action-skill-redaction".to_string(),
        conversation_model_context_items: Vec::new(),
        conversation_trace_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    });
    let mut record = PendingActionRecord {
        storage_id: pending_action_storage_id("run-skill-redaction", "action-skill-redaction"),
        snapshot: PendingAgentActionSnapshot {
            action_id: "action-skill-redaction".to_string(),
            action_type: "tool_call".to_string(),
            tool_name: "approval_tool".to_string(),
            tool_call_id: Some("action-skill-redaction".to_string()),
            run_id: "run-skill-redaction".to_string(),
            conversation_id: Some("conversation-skill-redaction".to_string()),
            assistant_message_id: Some("assistant-skill-redaction".to_string()),
            action: AgentProposedAction::ToolCall {
                call: AgentToolCall {
                    id: "action-skill-redaction".to_string(),
                    tool: "approval_tool".to_string(),
                    args: json!({}),
                    approval_status: AgentApprovalStatus::Required,
                    reason: None,
                },
            },
            created_at: 1,
            status: PendingActionStatus::Pending,
        },
        agent_input,
    };

    for status in [
        PendingActionStatus::Pending,
        PendingActionStatus::Approved,
        PendingActionStatus::Executing,
    ] {
        record.snapshot.status = status;
        let persisted = pending_storage_record(&record, 2).unwrap();
        assert!(persisted.agent_input_json.contains(MARKER));
        assert!(persisted.agent_input_json.contains(CATALOG_MARKER));
    }

    for status in [
        PendingActionStatus::Completed,
        PendingActionStatus::Rejected,
        PendingActionStatus::Cancelled,
        PendingActionStatus::Failed,
    ] {
        record.snapshot.status = status;
        let persisted = pending_storage_record(&record, 3).unwrap();
        assert!(!persisted.agent_input_json.contains(MARKER));
        assert!(!persisted.agent_input_json.contains(CATALOG_MARKER));
        assert!(!persisted.agent_input_json.contains("secret"));
        let restored = PersistedAgentResumeInput::decode(&persisted.agent_input_json)
            .unwrap()
            .agent_input;
        let activation = restored.skill_activation.unwrap();
        assert!(restored.skill_discovery.is_none());
        assert_eq!(activation.activation_revision, "activation-revision");
        assert_eq!(activation.skills.len(), 1);
        assert_eq!(
            activation.skills[0].id,
            "bundled:application:test-bundled-skill"
        );
        assert_eq!(activation.skills[0].revision, "package-revision");
        assert_eq!(activation.skills[0].source, "bundled:application");
        assert!(activation.skills[0].instructions.is_empty());
        let checkpoint = restored.resume_checkpoint.unwrap();
        assert!(checkpoint.context_items[0].content.is_empty());
        assert_eq!(
            checkpoint.context_items[0].sources,
            vec!["skill_instructions"]
        );
        assert_eq!(
            checkpoint.context_items[0].origin.as_ref().unwrap().id,
            "bundled:application:test-bundled-skill"
        );
        assert_eq!(checkpoint.context_items[1].content, "");
        assert_eq!(checkpoint.context_items[1].sources, vec!["skill_catalog"]);
        assert_eq!(
            checkpoint.context_items[2].content,
            "NON_SKILL_CHECKPOINT_CONTENT"
        );
        assert_eq!(
            checkpoint.extension_snapshots[0].state["discovery"],
            serde_json::Value::Null
        );
    }

    record.snapshot.status = PendingActionStatus::Pending;
    assert!(pending_storage_record(&record, 4)
        .unwrap()
        .agent_input_json
        .contains(MARKER));
    assert!(pending_storage_record(&record, 4)
        .unwrap()
        .agent_input_json
        .contains(CATALOG_MARKER));
}

#[tokio::test]
async fn pre_runtime_continuation_failure_terminalizes_turn_and_releases_occupancy() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let run_id = "pre-runtime-continuation-failure-run";
    let conversation_id = "pre-runtime-continuation-failure-conversation";
    let assistant_message_id = "pre-runtime-continuation-failure-assistant";
    let call = AgentToolCall {
        id: "pre-runtime-continuation-failure-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);

    let service = AgentService::new(Arc::clone(&storage));
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input.clone(),
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    let approved = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .persist_pending_target_status(&approved, PendingActionStatus::Completed)
        .unwrap();
    service.register_usage_context(
        run_id,
        AgentRunUsageContext {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            project_id: None,
            model_id: "test-model".to_string(),
            model_name: "Test Model".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );

    // The pending row contains a valid frozen checkpoint. Corrupt only the reconstructed
    // continuation so restoration fails after the exact durable Turn owner has been acquired,
    // but before any Runtime/model request can start.
    let mut resumed_input = agent_input;
    resumed_input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .extension_snapshots
        .push(mycopilot_core::AgentExtensionSnapshot {
            extension_id: "skills".to_string(),
            version: u32::MAX,
            state: json!({}),
        });
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .run_action_continuation(
            approved,
            resumed_input,
            notifications,
            PendingActionStatus::Completed,
            None,
        )
        .await;
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();

    let pending_status: String = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_status, "failed", "events: {events:#?}");
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert_eq!(trace.run_id, run_id);
    assert_eq!(trace.conversation_id, conversation_id);
    assert!(matches!(
        trace.items.first(),
        Some(ConversationTurnTraceItem::ToolCall { call_id, .. }) if call_id == &call.id
    ));
    assert!(trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
    )));
    let conversation = storage.load_conversation(conversation_id).unwrap().unwrap();
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == assistant_message_id)
        .unwrap();
    assert_eq!(assistant.status.as_deref(), Some("error"));
    assert!(
        assistant.content.contains("Skill extension version"),
        "unexpected terminal assistant content: {}",
        assistant.content
    );
    let usage = storage
        .load_agent_usage_for_owner(run_id, conversation_id, assistant_message_id)
        .unwrap()
        .expect("pre-Runtime failure must settle the existing run Usage owner");
    assert_eq!(usage.status.as_deref(), Some("failed"));
    assert!(usage
        .error
        .as_deref()
        .is_some_and(|error| error.contains("Skill extension version")));

    assert!(!service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
    service
        .reserve_conversation_turn(conversation_id, "next-run", "next-assistant")
        .expect("a durable terminal continuation must admit the next Turn");
    service.release_conversation_turn_if_current(conversation_id, "next-run");

    let error = events
        .iter()
        .find(|event| event["params"]["type"] == "error")
        .expect("terminal failure must emit an error event");
    assert_eq!(
        error["params"]["code"],
        "skill_resource_snapshot_unavailable"
    );
    assert_eq!(error["params"]["recoverable"], false);
    let done = events
        .iter()
        .find(|event| event["params"]["type"] == "done")
        .expect("terminal failure must emit a Done event");
    assert_eq!(done["params"]["status"], "failed");
    assert_eq!(done["params"]["success"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_approval_continuation_persists_waiting_to_running_before_runtime() {
    struct ApprovalRecoveryClock(std::sync::atomic::AtomicI64);

    impl crate::application::agent_dispatcher::AgentDispatcherClock for ApprovalRecoveryClock {
        fn now_ms(&self) -> i64 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_address = listener.local_addr().unwrap();
    let (provider_accepted, provider_entered) = tokio::sync::oneshot::channel();
    let provider = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        let _ = provider_accepted.send(());
        std::future::pending::<()>().await;
    });
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": format!("http://{provider_address}/v1/chat/completions"),
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-child-approval-root".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Approval root".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: "agent-child-approval-root".to_string(),
            conversation_id: "conversation-child-approval-root".to_string(),
            creation_request_id: "ensure-child-approval-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-foreign-approval-root".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Foreign approval root".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: "agent-foreign-approval-root".to_string(),
            conversation_id: "conversation-foreign-approval-root".to_string(),
            creation_request_id: "ensure-foreign-approval-root".to_string(),
            task_name: "Foreign root".to_string(),
        })
        .unwrap();
    let child = storage
        .create_child_agent(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: "agent-child-approval-root".to_string(),
            creation_request_id: "spawn-child-approval".to_string(),
            task_name: "approval_review".to_string(),
            task: "Pause for approval, then continue.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();
    let admitted_at = mycopilot_core::storage::now_ms();
    let claimed = storage
        .claim_next_dispatchable_agent_wake_at("approval-host", admitted_at)
        .unwrap()
        .unwrap();
    let run_id = "child-approval-continuation-run";
    let assistant_message_id = "child-approval-continuation-assistant";
    let call = AgentToolCall {
        id: test_mcp_call_id("child-approval-continuation-call"),
        tool: "apply_patch".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let provenance = AgentToolIdentity::Builtin {
        tool_name: call.tool.clone(),
    };
    let context = AgentRunContext {
        conversation_id: Some(child.agent.conversation_id.clone()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: Some(child.collaboration_identity.clone()),
    };
    agent_input.context = Some(context.clone());
    agent_input.assistant_message_id = Some(assistant_message_id.to_string());
    // Freeze the exact Tool authority through the same Host projection used by Runtime instead
    // of copying revision hashes into this durable approval fixture.
    let checkpoint_tool_set = {
        let projection_service =
            AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
                Arc::clone(&storage),
                None,
                1,
            )
            .unwrap();
        projection_service
            .context_window_tool_projection(&agent_input, None)
            .unwrap()
            .tool_set_checkpoint()
    };
    let (conversation, revision) = storage
        .load_conversation_for_turn(&child.agent.conversation_id)
        .unwrap();
    let mut conversation = conversation.unwrap();
    conversation.messages.push(ChatMessageRecord {
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: String::new(),
        created_at: admitted_at + 1,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    conversation.updated_at = admitted_at + 1;
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: child.agent.conversation_id.clone(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: call.args.clone(),
            provenance: provenance.clone(),
            approval_status: call.approval_status,
            truncated: false,
        }],
    };
    storage
        .save_conversation_and_begin_turn_with_preloaded_agent_messages(
            conversation,
            revision,
            Some(&mycopilot_core::TrustedAgentWakeTurnAdmission {
                agent_id: child.agent.agent_id.clone(),
                wake_id: claimed.wake_id.clone(),
                claim_token: claimed.claim_token.clone().unwrap(),
                source_agent_message_id: claimed.source_agent_message_id.clone().unwrap(),
            }),
            &[claimed.source_agent_message_id.clone().unwrap()],
            &trace,
            admitted_at + 1,
            admitted_at + 1,
        )
        .unwrap();
    let mut durable_checkpoint =
        test_pending_resume_checkpoint_for_call(&storage, run_id, None, &call, provenance.clone());
    durable_checkpoint.tool_set = checkpoint_tool_set.clone();
    durable_checkpoint.collaboration_run_snapshot =
        Some(mycopilot_core::AgentCollaborationRunSnapshot::default());
    let checkpoint_model_context = durable_checkpoint.conversation_model_context_items;
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &checkpoint_model_context,
            admitted_at + 1,
            admitted_at + 1,
        )
        .unwrap();
    let waiting = storage
        .transition_agent_wake(
            &claimed.wake_id,
            mycopilot_core::AgentWakeStatus::Running,
            mycopilot_core::AgentWakeStatus::WaitingForApproval,
            claimed.claim_token.as_deref(),
        )
        .unwrap();

    let mut checkpoint =
        test_pending_resume_checkpoint_for_call(&storage, run_id, None, &call, provenance);
    checkpoint.run_context = Some(context.clone());
    checkpoint.tool_set = checkpoint_tool_set;
    checkpoint.collaboration_run_snapshot =
        Some(mycopilot_core::AgentCollaborationRunSnapshot::default());
    agent_input.resume_checkpoint = Some(checkpoint);
    agent_input.approval_decision = Some(AgentApprovalDecision {
        action_id: call.id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    agent_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({ "approved": true })),
            error: None,
        },
    });

    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        1,
    )
    .unwrap();
    service
        .store_pending_action(
            run_id,
            &child.agent.conversation_id,
            assistant_message_id,
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input.clone(),
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let (root_card_notifications, _root_card_receiver) = tokio::sync::mpsc::unbounded_channel();
    let direct_child_write =
        service.approve_action(run_id, &call.id, root_card_notifications.clone());
    assert!(
        direct_child_write
            .unwrap_err()
            .contains("子 Agent Conversation 是只读观察视图"),
        "a user approval must not use the child Conversation write API"
    );
    assert!(!service.cancel_run(run_id));
    assert!(service
        .reject_action(
            run_id,
            &call.id,
            Some("forged rejection".to_string()),
            root_card_notifications.clone(),
        )
        .unwrap_err()
        .contains("子 Agent Conversation 是只读观察视图"));
    assert!(service
        .cancel_action(run_id, &call.id)
        .unwrap_err()
        .contains("子 Agent Conversation 是只读观察视图"));
    let projected = service
        .list_root_projected_approvals("conversation-child-approval-root")
        .unwrap();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].approval_id, storage_id);
    assert_eq!(projected[0].source_agent_id, child.agent.agent_id);
    assert_eq!(projected[0].action.run_id, run_id);
    let unknown_error = service
        .decide_root_projected_approval(
            "conversation-child-approval-root",
            "missing-projected-approval",
            ProjectedApprovalDecision::Cancel,
            None,
            root_card_notifications.clone(),
        )
        .unwrap_err();
    let foreign_error = service
        .decide_root_projected_approval(
            "conversation-foreign-approval-root",
            &storage_id,
            ProjectedApprovalDecision::Cancel,
            None,
            root_card_notifications.clone(),
        )
        .unwrap_err();
    assert_eq!(unknown_error, "Approval is unavailable.");
    assert_eq!(foreign_error, unknown_error);
    let restarted_projection =
        AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
            Arc::clone(&storage),
            None,
            1,
        )
        .unwrap();
    let recovered = restarted_projection
        .list_root_projected_approvals("conversation-child-approval-root")
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].approval_id, storage_id);
    assert_eq!(recovered[0].source_agent_id, child.agent.agent_id);
    drop(restarted_projection);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    let approved = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .persist_pending_target_status(&approved, PendingActionStatus::Completed)
        .unwrap();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .commit_trace_snapshot_with_continuation(&approved, &agent_input, &notifications)
        .unwrap();
    let gate = service.turn_concurrency_gate();
    assert_eq!(gate.active(), 1);
    let (dispatcher_notifications, _dispatcher_receiver) = tokio::sync::mpsc::unbounded_channel();
    let dispatcher_store: Arc<dyn crate::application::agent_dispatcher::AgentDispatcherStore> =
        Arc::new(
            crate::application::agent_dispatcher::SqliteAgentDispatcherStore::new(Arc::clone(
                &storage,
            )),
        );
    let dispatcher_executor: Arc<
        dyn crate::application::agent_dispatcher::AgentWakeTurnExecutionPort,
    > = Arc::new(
        crate::application::agent_dispatcher::SharedAgentTurnExecutionPort::new(
            service.clone(),
            Arc::clone(&storage),
            dispatcher_notifications,
        )
        .with_fallback_poll_interval(Duration::from_millis(5)),
    );
    let dispatcher = crate::application::agent_dispatcher::AgentDispatcher::start_with_clock(
        dispatcher_store,
        dispatcher_executor,
        Arc::new(ApprovalRecoveryClock(std::sync::atomic::AtomicI64::new(
            waiting.lease_expires_at.unwrap(),
        ))),
        gate.clone(),
        crate::application::agent_dispatcher::AgentDispatcherConfig {
            global_concurrency_limit: 1,
            wake_lease_renew_interval: Duration::from_millis(20),
            idle_poll_interval: Duration::from_millis(5),
            shutdown_grace: Duration::from_millis(250),
        },
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if dispatcher.observed_waiting_for_approval(&child.agent.agent_id) == Some(true) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Dispatcher must recover and observe the durable approval wait");

    // The approved tool result is already durable before the continuation enters Runtime. This
    // mirrors the production action runner and lets the observer distinguish a live continuation
    // from a still-pending approval after the Wake CAS.
    service
        .transition_pending_status(&approved, PendingActionStatus::Completed)
        .unwrap();
    let duplicate = service
        .decide_root_projected_approval(
            "conversation-child-approval-root",
            &storage_id,
            ProjectedApprovalDecision::Approve,
            None,
            root_card_notifications,
        )
        .unwrap();
    assert!(!duplicate.accepted);
    assert_eq!(duplicate.status, "completed");
    assert_eq!(duplicate.run_id, run_id);
    let continuation_service = service.clone();
    let continuation = tokio::spawn(async move {
        continuation_service
            .run_action_continuation(
                approved,
                agent_input,
                notifications,
                PendingActionStatus::Completed,
                None,
            )
            .await;
    });
    tokio::time::timeout(Duration::from_secs(2), provider_entered)
        .await
        .expect("continuation must reach the deterministic Provider boundary")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if dispatcher.observed_waiting_for_approval(&child.agent.agent_id) == Some(false) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("durable Running transition must make shutdown see a live continuation");
    let resumed = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
    assert_eq!(resumed.status, mycopilot_core::AgentWakeStatus::Running);
    assert_eq!(resumed.status_revision, waiting.status_revision + 1);
    assert_eq!(resumed.run_id.as_deref(), Some(run_id));
    assert_eq!(
        resumed.assistant_message_id.as_deref(),
        Some(assistant_message_id)
    );

    let shutdown = dispatcher.shutdown().await.unwrap();
    assert_eq!(
        shutdown.cancellation_requested, 1,
        "shutdown must interrupt the resumed live Runtime, not preserve it as approval waiting"
    );
    tokio::time::timeout(Duration::from_secs(2), continuation)
        .await
        .expect("cancelled continuation must converge")
        .unwrap();
    let terminal = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
    assert_eq!(
        terminal.status,
        mycopilot_core::AgentWakeStatus::Interrupted
    );
    assert!(terminal.result_message_id.is_some());
    assert_eq!(gate.active(), 0);
    provider.abort();
}

#[tokio::test]
async fn pre_runtime_continuation_failure_cas_conflict_preserves_turn_for_recovery() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "pre-runtime-continuation-conflict-run";
    let conversation_id = "pre-runtime-continuation-conflict-conversation";
    let assistant_message_id = "pre-runtime-continuation-conflict-assistant";
    let call = AgentToolCall {
        id: "pre-runtime-continuation-conflict-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input.clone(),
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    let approved = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .persist_pending_target_status(&approved, PendingActionStatus::Completed)
        .unwrap();

    let mut resumed_input = agent_input;
    resumed_input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .extension_snapshots
        .push(mycopilot_core::AgentExtensionSnapshot {
            extension_id: "skills".to_string(),
            version: u32::MAX,
            state: json!({}),
        });
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .run_action_continuation(
            approved,
            resumed_input,
            notifications,
            // Deliberately stale: the durable executor target is completed. The terminalization
            // transaction must lose this CAS without altering any durable fact or Turn lease.
            PendingActionStatus::Rejected,
            None,
        )
        .await;

    let (status, target_status): (String, Option<String>) =
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(status, "approved");
    assert_eq!(target_status.as_deref(), Some("completed"));
    assert_eq!(
        storage
            .get_conversation_turn_trace(assistant_message_id)
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    let conversation = storage.load_conversation(conversation_id).unwrap().unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    assert!(service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
    assert!(service
        .reserve_conversation_turn(conversation_id, "conflicting-next-run", "next-assistant")
        .is_err());

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    let error = events
        .iter()
        .find(|event| event["params"]["type"] == "error")
        .expect("CAS conflict must emit a recoverable persistence error");
    assert_eq!(
        error["params"]["code"],
        "conversation_trace_persistence_failed"
    );
    assert_eq!(error["params"]["recoverable"], true);
    assert!(!events.iter().any(|event| event["params"]["type"] == "done"));
}

#[test]
fn missing_pending_transition_row_fails_closed_without_terminal_success() {
    const MARKER: &str = "MISSING_TRANSITION_SKILL_BODY";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-missing-row".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application/test-bundled-skill".to_string(),
            name: "test-bundled-skill".to_string(),
            revision: "package-missing-row".to_string(),
            source: "bundled:application".to_string(),
            instructions: MARKER.to_string(),
            source_bytes: u64::try_from(MARKER.len()).unwrap(),
            resources: None,
        }],
    });
    let call = AgentToolCall {
        id: "action-missing-transition-row".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-missing-transition-row",
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    service
        .store_pending_action(
            "run-missing-transition-row",
            "conversation-missing-transition-row",
            "assistant-missing-transition-row",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();
    assert!(storage.list_pending_agent_actions().unwrap()[0]
        .agent_input_json
        .contains(MARKER));

    // Reproduce a cross-boundary missing-row race: durable conversation deletion has removed
    // the pending row while this service instance still owns its pre-deletion memory snapshot.
    storage
        .delete_conversation("conversation-missing-transition-row")
        .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .approve_action("run-missing-transition-row", &call.id, notifications)
        .unwrap_err();
    assert!(error.contains("实际更新 0 条"));
    assert!(receiver.try_recv().is_err());
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|lock_error| lock_error.into_inner())
            [&pending_action_storage_id("run-missing-transition-row", &call.id)]
            .snapshot
            .status,
        PendingActionStatus::Pending
    );

    let reloaded = AgentService::new(storage);
    assert!(reloaded.list_pending_actions().is_empty());
}

#[test]
fn invalid_checkpoint_tool_call_is_rejected_before_pending_publication() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let call = AgentToolCall {
        id: "call-invalid-checkpoint".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({ "value": "trusted" }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-invalid-checkpoint".to_string(),
        pending_action_id: None,
        context_items: vec![mycopilot_core::AgentContextCheckpointItem {
            role: "assistant".to_string(),
            content: String::new(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![mycopilot_core::AgentContextCheckpointToolCall {
                id: call.id.clone(),
                name: "tampered_tool".to_string(),
                args: call.args.clone(),
                provider_identity: mycopilot_core::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: call.id.clone(),
                    runtime_call_id: call.id.clone(),
                },
            }],
            is_error: false,
            sources: vec!["tool_call".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            group: None,
            origin: None,
        }],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: call.id.clone(),
        conversation_model_context_items: Vec::new(),
        conversation_trace_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    });
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let error = service
        .store_pending_action(
            "run-invalid-checkpoint",
            "conversation-invalid-checkpoint",
            "assistant-invalid-checkpoint",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap_err();
    assert_eq!(
        error,
        "Pending action frozen Tool Call identity is inconsistent."
    );
    assert!(service.list_pending_actions().is_empty());
    assert!(storage.list_pending_agent_actions().unwrap().is_empty());
}

#[test]
fn cancel_finalize_failure_atomically_restores_pending_payload() {
    const MARKER: &str = "CANCEL_ROLLBACK_SKILL_BODY";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let call = AgentToolCall {
        id: "action-cancel-rollback".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-cancel-rollback".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application/test-bundled-skill".to_string(),
            name: "test-bundled-skill".to_string(),
            revision: "package-cancel-rollback".to_string(),
            source: "bundled:application".to_string(),
            instructions: MARKER.to_string(),
            source_bytes: u64::try_from(MARKER.len()).unwrap(),
            resources: None,
        }],
    });
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-cancel-rollback",
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    // These durable owner rows intentionally do not exist, forcing final trace persistence to
    // fail after the action has first transitioned to cancelled.
    service
        .store_pending_action(
            "run-cancel-rollback",
            "conversation-cancel-rollback-missing",
            "assistant-cancel-rollback-missing",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();

    assert!(service
        .cancel_action("run-cancel-rollback", &call.id)
        .is_err());
    assert_eq!(
        service.list_pending_actions()[0].status,
        PendingActionStatus::Pending
    );
    let persisted = storage.list_pending_agent_actions().unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, "pending");
    assert_eq!(persisted[0].target_status, None);
    assert!(persisted[0].agent_input_json.contains(MARKER));

    // The compensating transition must make the action reusable. A stale cancelled marker
    // would make this second attempt fail before finalization and strand the row.
    let second_error = service
        .cancel_action("run-cancel-rollback", &call.id)
        .unwrap_err();
    assert!(!second_error.contains("无法写入 cancelled 目标终态"));
    let persisted = storage.list_pending_agent_actions().unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, "pending");
    assert_eq!(persisted[0].target_status, None);
}

#[test]
fn cancel_usage_failure_rolls_back_message_trace_and_action_together() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-cancel-usage-failure".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Cancel usage failure".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-cancel-usage-failure".to_string(),
                role: "assistant".to_string(),
                content: THINKING_PLACEHOLDER.to_string(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    // The invalid usage owner is a deterministic fault injection: the usage insert violates
    // its foreign key only after message and trace writes have run inside the transaction.
    service.register_usage_context(
        "run-cancel-usage-failure",
        AgentRunUsageContext {
            conversation_id: "missing-usage-conversation".to_string(),
            assistant_message_id: "assistant-cancel-usage-failure".to_string(),
            run_id: "run-cancel-usage-failure".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    let call = AgentToolCall {
        id: "call-cancel-usage-failure".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-cancel-usage-failure",
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    service
        .store_pending_action(
            "run-cancel-usage-failure",
            "conversation-cancel-usage-failure",
            "assistant-cancel-usage-failure",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();

    assert!(service
        .cancel_action("run-cancel-usage-failure", &call.id)
        .is_err());
    let pending = storage.list_pending_agent_actions().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, "pending");
    assert_eq!(pending[0].target_status, None);
    assert!(storage
        .get_conversation_turn_trace("assistant-cancel-usage-failure")
        .unwrap()
        .is_none());
    let conversation = storage
        .load_conversation("conversation-cancel-usage-failure")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].content, THINKING_PLACEHOLDER);
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    assert!(storage
        .list_agent_tool_results_for_run("run-cancel-usage-failure", "approval_tool")
        .unwrap()
        .is_empty());
}

#[test]
fn cancelled_file_write_with_durable_rejection_never_rolls_back_to_pending() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-file-write-cancel-failure".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "File write cancel failure".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-file-write-cancel-failure".to_string(),
                role: "assistant".to_string(),
                content: THINKING_PLACEHOLDER.to_string(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    service.register_usage_context(
        "run-file-write-cancel-failure",
        AgentRunUsageContext {
            conversation_id: "missing-file-write-usage-owner".to_string(),
            assistant_message_id: "assistant-file-write-cancel-failure".to_string(),
            run_id: "run-file-write-cancel-failure".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    let action_id = "file-write-cancel-failure";
    storage
        .create_agent_file_draft(AgentFileDraftRecord {
            id: "draft-file-write-cancel-failure".to_string(),
            conversation_id: "conversation-file-write-cancel-failure".to_string(),
            project_id: None,
            run_id: "run-file-write-cancel-failure".to_string(),
            file_path: "cancelled.txt".to_string(),
            mode: "create".to_string(),
            status: "waiting_approval".to_string(),
            base_revision: None,
            base_content: String::new(),
            content: "hello".to_string(),
            additions: 1,
            deletions: 0,
            line_count: 1,
            byte_count: 5,
            chunk_count: 1,
            next_chunk_index: 1,
            stats_final: true,
            summary: None,
            final_action_id: Some(action_id.to_string()),
            created_at: 1,
            updated_at: 1,
            expires_at: i64::MAX,
        })
        .unwrap();
    let file_write = AgentFileWriteProposal {
        id: action_id.to_string(),
        draft_id: "draft-file-write-cancel-failure".to_string(),
        mode: AgentFileWriteMode::Create,
        file_path: "cancelled.txt".to_string(),
        base_revision: None,
        summary: None,
        additions: 1,
        deletions: 0,
        line_count: 1,
        byte_count: 5,
        approval_status: AgentApprovalStatus::Required,
    };
    let call = AgentToolCall {
        id: file_write.id.clone(),
        tool: "write_file".to_string(),
        args: json!({
            "phase": "finish",
            "draftId": file_write.draft_id,
            "summary": file_write.summary,
        }),
        approval_status: file_write.approval_status,
        reason: file_write.summary.clone(),
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-file-write-cancel-failure",
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    ));
    append_durable_pending_trace(
        &storage,
        "conversation-file-write-cancel-failure",
        "assistant-file-write-cancel-failure",
        "run-file-write-cancel-failure",
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            "run-file-write-cancel-failure",
            "conversation-file-write-cancel-failure",
            "assistant-file-write-cancel-failure",
            AgentProposedAction::FileWrite { file_write },
            agent_input,
        )
        .unwrap();

    let error = service
        .cancel_action("run-file-write-cancel-failure", action_id)
        .unwrap_err();
    assert!(error.contains("保持 executing"));
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|lock_error| lock_error.into_inner())
            [&pending_action_storage_id("run-file-write-cancel-failure", action_id,)]
            .snapshot
            .status,
        PendingActionStatus::Executing
    );
    assert_eq!(
        storage
            .get_agent_file_draft("draft-file-write-cancel-failure")
            .unwrap()
            .unwrap()
            .status,
        "rejected"
    );
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-file-write-cancel-failure")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    let conversation = storage
        .load_conversation("conversation-file-write-cancel-failure")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    let reconciled_at = mycopilot_core::storage::now_ms();
    let interrupted = storage
        .reconcile_interrupted_pending_agent_actions(reconciled_at)
        .unwrap();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].status, "executing");
    assert_eq!(interrupted[0].target_status.as_deref(), Some("cancelled"));
    assert!(storage.list_pending_agent_actions().unwrap().is_empty());
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-file-write-cancel-failure")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
}
