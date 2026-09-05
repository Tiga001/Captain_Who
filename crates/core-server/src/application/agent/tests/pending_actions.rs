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
    storage: Option<Arc<StorageService>>,
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
        identity: &mycopilot_core::AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        self.invalidations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(storage) = self.storage.as_ref() {
            storage
                .delete_mcp_approval_envelope(&identity.invocation_id)
                .map_err(AgentError::from)?;
        }
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
    input.model_config_id = Some(input.model.clone());
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
    let audit = storage
        .get_agent_action_audit(&storage_id)
        .unwrap()
        .expect("automatic external MCP journal keeps its durable audit");
    assert_eq!(audit.decision_source.as_deref(), Some("auto"));
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
fn startup_adopts_a_terminal_auto_mcp_result_when_its_dispatch_journal_lagged() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-terminal-adoption.sqlite");
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
    let run_id = "auto-mcp-terminal-adoption-run";
    let conversation_id = "auto-mcp-terminal-adoption-conversation";
    let assistant_message_id = "auto-mcp-terminal-adoption-assistant";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let mut action = auto_mcp_action(run_id, &action_id, &invocation_id, now);
    let AgentProposedAction::McpToolCall { approval } = &mut action else {
        unreachable!("fixture always creates an external MCP action")
    };
    // External MCP arguments live only in the authenticated payload envelope. The durable
    // ToolCall operation is its safe projection and cannot be used to recompute this digest.
    approval.identity.arguments_digest =
        mycopilot_core::mcp_tool_arguments_digest(&json!({ "private": "omitted" })).unwrap();
    seed_durable_mcp_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &action,
        now,
    );
    let mut journal = service
        .prepare_auto_mcp_action_journal(
            run_id,
            Some(conversation_id),
            Some(assistant_message_id),
            action.clone(),
            auto_mcp_agent_input(&storage, run_id, &action_id),
        )
        .unwrap();
    service.claim_auto_mcp_dispatch(&mut journal).unwrap();
    append_terminal_auto_mcp_outcome_unknown(
        &storage,
        &database_path,
        conversation_id,
        assistant_message_id,
        run_id,
        &action,
        now,
    );
    let trace_before = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace_before.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    drop(service);

    let invoker = Arc::new(RecoverableApprovalRaceInvoker::default());
    let restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    assert_eq!(restarted.reconcile_startup_mcp_actions().unwrap(), 1);
    assert!(restarted.list_pending_actions().is_empty());
    assert_eq!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "startup adoption must never replay the external invocation"
    );

    let durable = storage
        .get_pending_agent_action(&journal.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "failed");
    assert_eq!(durable.target_status.as_deref(), Some("failed"));
    assert_eq!(durable.action_json, "{}");
    assert_eq!(durable.agent_input_json, "{}");
    let trace_after = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(trace_after, trace_before);
    trace_after
        .validate_complete_model_context(
            &storage
                .get_conversation_model_context_log(assistant_message_id)
                .unwrap()
                .unwrap()
                .items,
        )
        .unwrap();
}

#[test]
fn startup_rejects_malformed_terminal_auto_mcp_result_instead_of_adopting_it() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-malformed-terminal.sqlite");
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
    let run_id = "auto-mcp-malformed-terminal-run";
    let conversation_id = "auto-mcp-malformed-terminal-conversation";
    let assistant_message_id = "auto-mcp-malformed-terminal-assistant";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let action = auto_mcp_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &action,
        now,
    );
    let mut journal = service
        .prepare_auto_mcp_action_journal(
            run_id,
            Some(conversation_id),
            Some(assistant_message_id),
            action.clone(),
            auto_mcp_agent_input(&storage, run_id, &action_id),
        )
        .unwrap();
    service.claim_auto_mcp_dispatch(&mut journal).unwrap();
    append_terminal_auto_mcp_outcome_unknown(
        &storage,
        &database_path,
        conversation_id,
        assistant_message_id,
        run_id,
        &action,
        now,
    );
    drop(service);

    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute(
            "UPDATE conversation_turn_trace_items
             SET item_json = json_set(item_json, '$.observation.contentOmitted', false)
             WHERE assistant_message_id = ?1 AND item_kind = 'tool_result'",
            [assistant_message_id],
        )
        .unwrap();

    let restarted =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    assert!(restarted.reconcile_startup_mcp_actions().is_err());
    let durable = storage
        .get_pending_agent_action(&journal.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "executing");
    assert!(durable.target_status.is_none());
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
            managed_office_script: None,
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
        "conversationTraceTruncated": false,
        "fileChangeRunGrantRef": null,
        "pendingFileObservation": null
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
    let (trace_operation, trace_truncated) = durable_test_tool_call_operation(call);
    checkpoint.conversation_trace_items = vec![ConversationTurnTraceItem::ToolCall {
        sequence: 0,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        operation: trace_operation,
        provenance,
        approval_status: call.approval_status,
        truncated: trace_truncated,
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
    checkpoint.conversation_trace_truncated = trace_truncated;
    checkpoint
}

fn durable_test_tool_call_operation(call: &AgentToolCall) -> (serde_json::Value, bool) {
    if call.tool == "apply_patch" {
        let operation = mycopilot_core::file_change_support::apply_patch_trace_operation(
            &call.args,
        )
        .expect("project test apply_patch call through the production durable Trace boundary");
        let truncated = operation != call.args;
        (operation, truncated)
    } else {
        (call.args.clone(), false)
    }
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
        "fileChangeProposals": [],
        "fileChanges": [],
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
    let (trace_operation, trace_truncated) = durable_test_tool_call_operation(call);
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: trace_truncated,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            provenance,
            operation: trace_operation,
            approval_status: call.approval_status,
            truncated: trace_truncated,
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

fn append_terminal_auto_mcp_outcome_unknown(
    storage: &StorageService,
    database_path: &std::path::Path,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    action: &AgentProposedAction,
    created_at: i64,
) {
    let AgentProposedAction::McpToolCall { approval } = action else {
        panic!("test helper requires an MCP action");
    };
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    let model_context_items = storage
        .get_conversation_model_context_log(assistant_message_id)
        .unwrap()
        .unwrap()
        .items;
    let next_sequence = trace
        .items
        .last()
        .map(ConversationTurnTraceItem::sequence)
        .unwrap_or(0)
        .checked_add(1)
        .unwrap();
    let snapshot = ConversationTraceSnapshot {
        items: trace.items,
        model_context_items,
        next_sequence,
        truncated: trace.truncated,
    };
    let result = mycopilot_core::mcp_tool_result_persistence_projection(&AgentToolResult {
        exact_archive_file: None,
        call_id: approval.identity.call_id.clone(),
        tool: approval.identity.provenance.model_tool_name.clone(),
        ok: false,
        result: Some(json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "status": "outcome_unknown",
            "outcome": "outcome_unknown",
            "code": "mcp.tool_outcome_unknown",
            "retryable": false,
            "external": true,
            "dispatchCertainty": "possibly_dispatched",
            "isError": true
        })),
        error: Some("The external MCP tool reported an error.".to_string()),
    });
    let mut terminal = mycopilot_core::terminal_conversation_trace_from_snapshot_with_tool_result(
        snapshot,
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Completed,
        "",
        &result,
    )
    .unwrap();
    terminal.trace.terminal_error = None;
    terminal
        .trace
        .validate_complete_model_context(&terminal.model_context_items)
        .unwrap();
    let usage = AgentUsageRecordInsert {
        id: format!("usage-{assistant_message_id}"),
        conversation_id: conversation_id.to_string(),
        message_id: assistant_message_id.to_string(),
        run_id: run_id.to_string(),
        project_id: None,
        model_id: "test-model".to_string(),
        model_name: "Test Model".to_string(),
        started_at: Some(created_at),
        completed_at: Some(created_at.saturating_add(1)),
        status: Some("completed".to_string()),
        error: None,
        created_at,
        input_tokens: None,
        output_tokens: None,
        output_thinking_tokens: None,
        total_tokens: None,
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        billable_request_count: 0,
        input_price: None,
        cached_input_price: None,
        output_price: None,
        estimated_cost: None,
    };
    storage
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            conversation_id,
            assistant_message_id,
            "",
            Some("sent"),
            "completed",
            &terminal.trace,
            Some(&terminal.model_context_items),
            created_at,
            created_at.saturating_add(1),
            Some(&usage),
            None,
        )
        .unwrap();
    let connection = rusqlite::Connection::open(database_path).unwrap();
    let raw: String = connection
        .query_row(
            "SELECT agent_run_json FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![conversation_id, assistant_message_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut run: Value = serde_json::from_str(&raw).unwrap();
    let invocation = run["mcpInvocations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|invocation| invocation["actionId"] == approval.identity.action_id)
        .unwrap();
    *invocation = json!({
        "actionId": approval.identity.action_id,
        "invocationId": approval.identity.invocation_id,
        "callId": approval.identity.call_id,
        "serverId": approval.identity.provenance.server_id,
        "serverDisplayName": approval.summary.server_display_name,
        "scope": approval.identity.provenance.scope,
        "rawToolName": approval.identity.provenance.raw_tool_name,
        "modelToolName": approval.identity.provenance.model_tool_name,
        "external": true,
        "state": "outcome_unknown",
        "dispatchCertainty": "possibly_dispatched",
        "outcome": "outcome_unknown",
        "errorCode": "mcp.tool_outcome_unknown",
        "durationMs": 1,
        "outputTruncated": false
    });
    connection
        .execute(
            "UPDATE messages SET agent_run_json = ?3
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![conversation_id, assistant_message_id, run.to_string()],
        )
        .unwrap();
}

fn predecessor_gate_records(
    storage: &StorageService,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    predecessor_status: PendingActionStatus,
) -> (PendingActionRecord, PendingActionRecord) {
    let predecessor_call = AgentToolCall {
        id: "predecessor-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let successor_call = AgentToolCall {
        id: "successor-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let provenance = AgentToolIdentity::Builtin {
        tool_name: "approval_tool".to_string(),
    };
    seed_durable_pending_owner(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &predecessor_call,
        provenance.clone(),
        1,
    );

    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: predecessor_call.id.clone(),
                tool: predecessor_call.tool.clone(),
                provenance: provenance.clone(),
                operation: predecessor_call.args.clone(),
                approval_status: predecessor_call.approval_status,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: predecessor_call.id.clone(),
                tool: predecessor_call.tool.clone(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({ "status": "completed" }),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 2,
                call_id: successor_call.id.clone(),
                tool: successor_call.tool.clone(),
                provenance: provenance.clone(),
                operation: successor_call.args.clone(),
                approval_status: successor_call.approval_status,
                truncated: false,
            },
        ],
    };
    let model_context = vec![
        ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: predecessor_call.id.clone(),
                name: predecessor_call.tool.clone(),
                args: predecessor_call.args.clone(),
                provider_identity: AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: predecessor_call.id.clone(),
                    runtime_call_id: predecessor_call.id.clone(),
                },
            }],
            is_error: false,
        },
        ConversationModelContextItem {
            sequence: 1,
            ordinal: 0,
            role: "tool".to_string(),
            content: json!({ "status": "completed" }).to_string(),
            tool_call_id: Some(predecessor_call.id.clone()),
            tool_calls: Vec::new(),
            is_error: false,
        },
        ConversationModelContextItem {
            sequence: 2,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: successor_call.id.clone(),
                name: successor_call.tool.clone(),
                args: successor_call.args.clone(),
                provider_identity: AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: successor_call.id.clone(),
                    runtime_call_id: successor_call.id.clone(),
                },
            }],
            is_error: false,
        },
    ];
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &model_context,
            2,
            2,
        )
        .unwrap();

    let mut base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(storage, &mut base_input);
    let context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    base_input.context = Some(context.clone());

    let mut predecessor_input = base_input.clone();
    let mut predecessor_checkpoint = test_pending_resume_checkpoint_for_call(
        storage,
        run_id,
        None,
        &predecessor_call,
        provenance.clone(),
    );
    predecessor_checkpoint.run_context = Some(context.clone());
    predecessor_input.resume_checkpoint = Some(predecessor_checkpoint);
    let predecessor = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, &predecessor_call.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: predecessor_call.id.clone(),
            action_type: "tool_call".to_string(),
            tool_name: predecessor_call.tool.clone(),
            tool_call_id: Some(predecessor_call.id.clone()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action: AgentProposedAction::ToolCall {
                call: predecessor_call,
            },
            created_at: 1,
            status: predecessor_status,
        },
        agent_input: predecessor_input,
    };

    let mut successor_input = base_input;
    let mut successor_checkpoint =
        test_pending_resume_checkpoint_for_call(storage, run_id, None, &successor_call, provenance);
    successor_checkpoint.run_context = Some(context);
    successor_checkpoint.conversation_trace_items = trace.items;
    successor_checkpoint.conversation_model_context_items = model_context;
    successor_checkpoint.next_conversation_trace_sequence = 3;
    successor_input.resume_checkpoint = Some(successor_checkpoint);
    let successor = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, &successor_call.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: successor_call.id.clone(),
            action_type: "tool_call".to_string(),
            tool_name: successor_call.tool.clone(),
            tool_call_id: Some(successor_call.id.clone()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action: AgentProposedAction::ToolCall {
                call: successor_call,
            },
            // Both approvals belong to one logical Run and therefore share its authoritative
            // usage start; trace sequence, not wall-clock creation order, proves dependency.
            created_at: 1,
            status: PendingActionStatus::Pending,
        },
        agent_input: successor_input,
    };
    (predecessor, successor)
}

#[test]
fn successor_approval_is_blocked_until_its_durable_predecessor_settles() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    // Construct the service before seeding the synthetic crash window so startup reconciliation
    // cannot repair it for this live-process gate test.
    let service = AgentService::new(Arc::clone(&storage));
    let (predecessor, successor) = predecessor_gate_records(
        &storage,
        "run-predecessor-gate",
        "conversation-predecessor-gate",
        "assistant-predecessor-gate",
        PendingActionStatus::Executing,
    );
    let mut predecessor_row = pending_storage_record(&predecessor, 3).unwrap();
    predecessor_row.target_status = Some("completed".to_string());
    storage.store_pending_agent_action(predecessor_row).unwrap();
    storage
        .store_pending_agent_action(pending_storage_record(&successor, 3).unwrap())
        .unwrap();
    {
        let mut pending = service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        pending.insert(predecessor.storage_id.clone(), predecessor.clone());
        pending.insert(successor.storage_id.clone(), successor.clone());
    }

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .queue_action_continuation(
            &successor.snapshot.run_id,
            &successor.snapshot.action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
        .unwrap_err();
    assert!(error.contains("前置工具结果尚未完成持久化结算"));
    assert_eq!(
        storage
            .get_pending_agent_action(&successor.storage_id)
            .unwrap()
            .unwrap()
            .status,
        "pending"
    );

    // Durable status wins over stale process-local state. A commit-unknown terminal CAS must not
    // leave B permanently blocked merely because this Host still remembers A as executing.
    storage
        .transition_pending_agent_action(&predecessor.storage_id, "executing", "completed", "{}", 4)
        .unwrap();
    assert!(!storage
        .pending_agent_action_has_unsettled_predecessor(
            &successor.storage_id,
            std::slice::from_ref(&predecessor.snapshot.action_id),
        )
        .unwrap());
}

#[test]
fn restart_loaded_pending_map_still_blocks_a_proven_successor() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let (predecessor, successor) = predecessor_gate_records(
        &storage,
        "run-restarted-predecessor-gate",
        "conversation-restarted-predecessor-gate",
        "assistant-restarted-predecessor-gate",
        PendingActionStatus::Pending,
    );
    storage
        .store_pending_agent_action(pending_storage_record(&predecessor, 3).unwrap())
        .unwrap();
    storage
        .store_pending_agent_action(pending_storage_record(&successor, 3).unwrap())
        .unwrap();

    let reloaded = AgentService::new(Arc::clone(&storage));
    let loaded = reloaded
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(loaded.contains_key(&predecessor.storage_id));
    assert!(loaded.contains_key(&successor.storage_id));
    drop(loaded);

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = reloaded
        .queue_action_continuation(
            &successor.snapshot.run_id,
            &successor.snapshot.action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
        .unwrap_err();
    assert!(error.contains("前置工具结果尚未完成持久化结算"));
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
                        "fileChangeProposals": [],
                        "fileChanges": [],
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
    let mycopilot_core::ProviderProfileConfig::V1(config) = &mut profile else {
        unreachable!("legacy DeepSeek constructor must produce schema v1")
    };
    config.reasoning = mycopilot_core::ReasoningPolicy {
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
                created_at, position
             ) VALUES (?1, ?2, 'assistant', 'visible', 'pending', NULL, 1, 0)",
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

#[tokio::test]
async fn provider_continuation_preflight_accepts_decision_but_blocks_mcp_dispatch() {
    for scenario in ["empty", "missing", "tampered"] {
        let fixture = tempdir().unwrap();
        let database_path = fixture
            .path()
            .join(format!("provider-preflight-{scenario}.sqlite"));
        let credentials =
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
        let storage = Arc::new(
            StorageService::open_with_model_credentials(&database_path, credentials.clone())
                .unwrap(),
        );
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
        let mycopilot_core::ProviderProfileConfig::V1(config) = &mut deepseek_profile else {
            unreachable!("legacy DeepSeek constructor must produce schema v1")
        };
        config.reasoning = mycopilot_core::ReasoningPolicy {
            mode: mycopilot_core::ReasoningMode::Disabled,
            effort: mycopilot_core::ReasoningEffort::ProviderDefault,
        };
        provider_settings.models[0].provider_profile_config = deepseek_profile;
        storage.save_model_settings(provider_settings).unwrap();
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
        let action = test_mcp_pending_action(
            &run_id,
            &action_id,
            &invocation_id,
            mycopilot_core::storage::now_ms(),
        );
        let AgentProposedAction::McpToolCall { approval } = &action else {
            unreachable!("fixture always creates an MCP approval");
        };
        let mut input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "test-token",
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "messages": []
        }))
        .unwrap();
        let run_context = mycopilot_core::AgentRunContext {
            conversation_id: Some(conversation_id.clone()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: mycopilot_core::AgentPermissions::default(),
            collaboration_identity: None,
        };
        input.context = Some(run_context.clone());
        let mut checkpoint = test_pending_resume_checkpoint_for_call(
            &storage,
            &run_id,
            Some(&action_id),
            &approval.call,
            AgentToolIdentity::Mcp {
                provenance: approval.identity.provenance.clone(),
            },
        );
        checkpoint.run_context = Some(run_context);
        input.resume_checkpoint = Some(checkpoint);
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
        seed_durable_mcp_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &action,
            mycopilot_core::storage::now_ms(),
        );
        assert!(service
            .store_pending_action(
                &run_id,
                &conversation_id,
                &assistant_message_id,
                action,
                input,
            )
            .unwrap());
        drop(service);
        drop(storage);

        let reopened_storage = Arc::new(
            StorageService::open_with_model_credentials(&database_path, credentials.clone())
                .unwrap(),
        );
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
        let output = restarted
            .approve_action(&run_id, &action_id, notifications)
            .unwrap();
        assert_eq!(output.status, "failed", "unexpected result for {scenario}");
        assert_eq!(
            output.agent_output.status,
            AgentRunStatus::Running,
            "the accepted ToolResult continues through the normal settlement chain"
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

    let invoker = Arc::new(InvalidatingMcpInvoker {
        storage: Some(Arc::clone(&storage)),
        ..InvalidatingMcpInvoker::default()
    });
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
fn startup_preserves_pending_mcp_tickets_while_pruning_expired_and_orphaned_payloads() {
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

    // Headless child Turns do not have a Renderer maintaining `agent_run_json`. The durable
    // approval ticket must survive independently from its short-lived sealed payload envelope.
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
    let pending = restarted.list_pending_actions();
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().any(|action| action.run_id == live_run_id));
    assert!(pending.iter().any(|action| action.run_id == expired_run_id));
    assert_eq!(
        storage
            .list_recoverable_agent_actions_after_reconciliation()
            .unwrap()
            .len(),
        2
    );
    assert!(storage
        .load_mcp_approval_envelope(&live_invocation_id)
        .unwrap()
        .is_some());
    assert!(storage
        .load_mcp_approval_envelope(&expired_invocation_id)
        .unwrap()
        .is_none());
    assert!(storage
        .load_mcp_approval_envelope(&orphan_invocation_id)
        .unwrap()
        .is_none());
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
        let action_created_at = if status == PendingActionStatus::Approved {
            now.saturating_sub(120_000)
        } else {
            now
        };
        let action =
            test_mcp_pending_action(&run_id, &action_id, &invocation_id, action_created_at);
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
            McpApprovalStartupPayloadState::Expired,
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

#[tokio::test]
async fn expired_mcp_approval_accepts_one_decision_and_settles_execution_normally() {
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

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let accepted = service
        .approve_action(run_id, &action_id, notifications.clone())
        .unwrap();
    assert_eq!(accepted.status, "approved");
    assert!(service
        .approve_action(run_id, &action_id, notifications)
        .is_err());

    for _ in 0..100 {
        let status = storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .map(|record| record.status);
        if matches!(status.as_deref(), Some("failed" | "completed")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        invoker
            .invalidations
            .load(std::sync::atomic::Ordering::SeqCst)
            >= 1,
        "payload cleanup is idempotent; rows={:?}",
        storage.list_pending_agent_actions().unwrap()
    );
    assert!(service.list_pending_actions().is_empty());

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let terminal: (String, Option<String>) = connection
        .query_row(
            "SELECT pending.status, audit.error
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_ne!(terminal.0, "pending");
    assert_ne!(terminal.1.as_deref(), Some("mcp.approval_payload_expired"));
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
    let invoker = Arc::new(InvalidatingMcpInvoker {
        storage: Some(Arc::clone(&storage)),
        ..InvalidatingMcpInvoker::default()
    });
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
            terminalized_before_dispatch: 1,
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
        let terminal: (String, String, String, String, Option<String>) = connection
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
        if *status == PendingActionStatus::Pending {
            assert_eq!(terminal.0, "pending");
            assert_ne!(terminal.1, "{}");
            assert_ne!(terminal.2, "{}");
            assert_ne!(terminal.3, "{}");
        } else {
            assert_eq!(terminal.0, "failed");
            assert_eq!(terminal.1, "{}");
            assert_eq!(terminal.2, "{}");
            assert_eq!(terminal.3, "{}");
            assert_eq!(
                terminal.4.as_deref(),
                Some(if *status == PendingActionStatus::Executing {
                    "mcp.tool_outcome_unknown"
                } else {
                    "mcp.approval_policy_denied"
                })
            );
        }
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
    assert_eq!(filtered_summary.terminalized_before_dispatch, 0);
    assert_eq!(filtered_summary.terminalized_outcome_unknown, 0);
    assert_eq!(filtered_summary.payload_invalidation_attempts, 1);
    assert_eq!(filtered_summary.payload_invalidation_failures, 0);
    let filtered_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&filtered.0],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(filtered_status, "pending");
    assert!(storage
        .load_mcp_approval_envelope(&filtered.1)
        .unwrap()
        .is_none());

    let in_memory = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(in_memory.len(), 5);
    assert!(in_memory.contains_key(&selected[0].1 .0));
    assert!(in_memory.contains_key(&filtered.0));
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
    let invoker = Arc::new(InvalidatingMcpInvoker {
        storage: Some(Arc::clone(&storage)),
        ..InvalidatingMcpInvoker::default()
    });
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
    assert_eq!(summary.terminalized_before_dispatch, 1);
    assert_eq!(summary.terminalized_outcome_unknown, 1);
    assert_eq!(summary.payload_invalidation_attempts, 3);
    assert_eq!(summary.payload_invalidation_failures, 0);

    let connection = rusqlite::Connection::open(database_path).unwrap();
    for (status, (storage_id, invocation_id)) in &prior {
        let terminal: (String, Option<String>) = connection
            .query_row(
                "SELECT pending.status, audit.error
                 FROM agent_pending_actions pending
                 JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                 WHERE pending.action_id = ?1",
                [storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        if *status == PendingActionStatus::Pending {
            assert_eq!(terminal.0, "pending");
        } else {
            assert_eq!(terminal.0, "failed");
            assert_eq!(
                terminal.1.as_deref(),
                Some(if *status == PendingActionStatus::Executing {
                    "mcp.tool_outcome_unknown"
                } else {
                    "mcp.approval_policy_denied"
                })
            );
        }
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
fn direct_file_change_execution_is_private_to_renderer_but_durable_for_restart() {
    const PRIVATE_WHOLE_FILE_MARKER: &str = "PRIVATE_FILE_CHANGE_CONTENT_MUST_STAY_HOST_SIDE";

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

    let filler = (1..=12)
        .map(|index| format!("unchanged line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let base_content = format!("{PRIVATE_WHOLE_FILE_MARKER}\n{filler}\npublic status: before\n");
    let target_content = format!("{PRIVATE_WHOLE_FILE_MARKER}\n{filler}\npublic status: after\n");
    let run_id = "run-direct-file-change";
    let conversation_id = "conversation-direct-file-change";
    let call_id = "call-direct-file-change";
    let canonical_target = fixture.path().join("report.txt");
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("report.txt", canonical_target.to_str().unwrap()),
        Some(&base_content),
        Some(&target_content),
        AgentApprovalStatus::Required,
    );
    let AgentProposedAction::FileChange { file_change } = &action else {
        panic!("fixture must produce a FileChange action");
    };
    assert!(
        !file_change
            .inline_diff
            .as_ref()
            .unwrap()
            .patch
            .contains(PRIVATE_WHOLE_FILE_MARKER),
        "the presentation diff fixture must not itself contain the private whole-file marker"
    );

    let renderer_event = agent_event_notification(AgentEvent::FileChangeProposed {
        run_id: run_id.to_string(),
        file_change: file_change.clone(),
    });
    let observer_event = child_observer_event_notification(
        &valid_resume_collaboration_identity(),
        run_id,
        "assistant-direct-file-change",
        AgentEvent::FileChangeProposed {
            run_id: run_id.to_string(),
            file_change: file_change.clone(),
        },
    );
    assert_eq!(renderer_event["method"], AGENT_EVENT_NAME);
    let pending_snapshot = PendingAgentActionSnapshot {
        action_id: call_id.to_string(),
        action_type: "file_change".to_string(),
        tool_name: "apply_patch".to_string(),
        tool_call_id: Some(call_id.to_string()),
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some("assistant-direct-file-change".to_string()),
        action: action.clone(),
        created_at: 1,
        status: PendingActionStatus::Pending,
    };
    let renderer_pending = serde_json::to_value(&pending_snapshot).unwrap();

    assert!(renderer_event["params"]["fileChange"]
        .get("execution")
        .is_none());
    assert!(observer_event["params"]["event"]["fileChange"]
        .get("execution")
        .is_none());
    assert!(renderer_pending["action"]["fileChange"]
        .get("execution")
        .is_none());
    for (boundary, projection) in [
        ("agent.event", &renderer_event),
        ("agent observer event", &observer_event),
        ("pending snapshot", &renderer_pending),
    ] {
        let encoded = serde_json::to_string(projection).unwrap();
        for forbidden in [
            "\"execution\"",
            "\"baseContent\"",
            "\"targetContent\"",
            PRIVATE_WHOLE_FILE_MARKER,
        ] {
            assert!(
                !encoded.contains(forbidden),
                "{boundary} leaked Host-private FileChange material `{forbidden}`"
            );
        }
    }

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
        run_id,
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    ));
    let record = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, call_id),
        snapshot: pending_snapshot,
        agent_input,
    };
    let durable = pending_storage_record(&record, 2).unwrap();
    let durable_action: Value = serde_json::from_str(&durable.action_json).unwrap();
    assert_eq!(
        durable_action["fileChange"]["execution"]["baseContent"],
        base_content
    );
    assert_eq!(
        durable_action["fileChange"]["execution"]["targetContent"],
        target_content
    );
    let restored: AgentProposedAction = serde_json::from_str(&durable.action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: restored,
    } = restored
    else {
        panic!("durable action must round-trip as a FileChange");
    };
    restored.execution.validate().unwrap();
    assert_eq!(restored.execution.source_call_id, call_id);
    assert_eq!(restored.execution.conversation_id, conversation_id);
    assert_eq!(restored.execution.run_id, run_id);
    assert_eq!(
        restored.execution.base_content.as_deref(),
        Some(base_content.as_str())
    );
    assert_eq!(
        restored.execution.target_content.as_deref(),
        Some(target_content.as_str())
    );
}

fn seed_interrupted_manual_file_change(
    storage: &Arc<StorageService>,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    action: AgentProposedAction,
    call: &AgentToolCall,
) -> PendingActionRecord {
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(storage, &mut agent_input);
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    let storage_id = pending_action_storage_id(run_id, &call.id);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        storage,
        run_id,
        Some(&storage_id),
        call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    ));
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    freeze_test_pending_provider_configuration(storage, &mut agent_input);
    seed_durable_pending_owner(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    let record = PendingActionRecord {
        storage_id,
        snapshot: PendingAgentActionSnapshot {
            action_id: call.id.clone(),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_call_id: Some(call.id.clone()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action,
            created_at: 1,
            status: PendingActionStatus::Executing,
        },
        agent_input,
    };
    storage
        .store_pending_agent_action(pending_storage_record(&record, 2).unwrap())
        .unwrap();
    assert!(storage
        .insert_agent_action_audit_if_absent(action_audit_record(
            &record, None, "pending", None, None, None, None, None, None,
        ))
        .unwrap());
    record
}

#[test]
fn startup_reconciles_published_manual_direct_file_change_without_replaying_it() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let run_id = "run-manual-direct-crash";
    let conversation_id = "conversation-manual-direct-crash";
    let assistant_message_id = "assistant-manual-direct-crash";
    let call_id = "call-manual-direct-crash";
    let target_content = "published before the durable receipt\n";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join("manual-recovered.txt");
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("manual-recovered.txt", target_path.to_str().unwrap()),
        None,
        Some(target_content),
        AgentApprovalStatus::Required,
    );

    // This is the crash window: publication succeeded, but both pending and manual audit still
    // contain the prepared credential and no terminal ToolResult exists.
    std::fs::write(&target_path, target_content).unwrap();
    let metadata_before = std::fs::metadata(&target_path).unwrap();
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let pending_action_id = pending_action_storage_id(run_id, call_id);
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&pending_action_id),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    );
    checkpoint.run_context = agent_input.context.clone();
    agent_input.resume_checkpoint = Some(checkpoint);
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    let record = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, call_id),
        snapshot: PendingAgentActionSnapshot {
            action_id: call_id.to_string(),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_call_id: Some(call_id.to_string()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action,
            created_at: 1,
            status: PendingActionStatus::Executing,
        },
        agent_input,
    };
    storage
        .store_pending_agent_action(pending_storage_record(&record, 2).unwrap())
        .unwrap();
    assert!(storage
        .insert_agent_action_audit_if_absent(action_audit_record(
            &record, None, "pending", None, None, None, None, None, None,
        ))
        .unwrap());

    let candidates = storage
        .list_interrupted_agent_actions_for_host_reconciliation()
        .unwrap();
    assert_eq!(candidates.len(), 1);
    PersistedAgentResumeInput::decode(&candidates[0].agent_input_json)
        .expect("manual crash fixture must retain a current resume input");
    let prepared: AgentProposedAction = serde_json::from_str(&candidates[0].action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: prepared,
    } = prepared
    else {
        panic!("manual crash fixture must retain a FileChange");
    };
    prepared.execution.validate().unwrap();
    let resolved = mycopilot_core::file_change::FileChangePathPolicy::new(None, true)
        .resolve(&prepared.execution.canonical_target)
        .unwrap();
    let plan = mycopilot_core::file_change::FileChangePlan::from_direct_binding(
        &prepared.execution,
        prepared.inline_diff.as_ref().unwrap().patch.clone(),
    )
    .unwrap();
    assert_eq!(
        mycopilot_core::file_change::FileChangeCommitter
            .reconcile(&resolved, &plan, prepared.execution.delete_journal.as_ref())
            .unwrap(),
        mycopilot_core::file_change::FileChangeReconciliation::AlreadyApplied
    );

    let reconciled_at = mycopilot_core::storage::now_ms();
    assert_eq!(
        reconcile_interrupted_file_changes(&storage, reconciled_at).unwrap(),
        1,
        "the Direct startup pre-pass must recognize the already-published target"
    );
    let after_direct = storage
        .get_pending_agent_action(&record.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(after_direct.status, "executing");
    assert_eq!(after_direct.target_status.as_deref(), Some("completed"));
    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("published Direct FileChange must reconcile from its target digest");

    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        target_content
    );
    let metadata_after = std::fs::metadata(&target_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(metadata_after.dev(), metadata_before.dev());
        assert_eq!(metadata_after.ino(), metadata_before.ino());
    }
    #[cfg(not(unix))]
    let _ = (metadata_before, metadata_after);

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let (pending_status, pending_action_json, audit_status, audit_action_json): (
        String,
        String,
        String,
        String,
    ) = connection
        .query_row(
            "SELECT pending.status, pending.action_json, audit.status, audit.action_json
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&record.storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(pending_status, "completed");
    assert_eq!(audit_status, "completed");
    assert_eq!(pending_action_json, audit_action_json);
    let committed: AgentProposedAction = serde_json::from_str(&pending_action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = committed else {
        panic!("recovered pending action must remain a FileChange");
    };
    assert_eq!(
        file_change.execution.transaction.status,
        mycopilot_core::file_change::FileChangeStatus::AlreadyApplied
    );
    assert!(file_change.execution.receipt.is_some());
}

#[test]
fn startup_file_change_reconciliation_types_base_divergence_and_unreadable_targets() {
    #[derive(Clone, Copy)]
    enum Case {
        Base,
        Divergent,
        #[cfg(unix)]
        Unreadable,
    }
    let mut cases = vec![Case::Base, Case::Divergent];
    #[cfg(unix)]
    cases.push(Case::Unreadable);

    for (index, case) in cases.into_iter().enumerate() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let run_id = format!("run-file-change-recovery-{index}");
        let conversation_id = format!("conversation-file-change-recovery-{index}");
        let assistant_message_id = format!("assistant-file-change-recovery-{index}");
        let call_id = format!("call-file-change-recovery-{index}");
        let canonical_fixture = std::fs::canonicalize(fixture.path()).unwrap();
        let parent = canonical_fixture.join("observed");
        std::fs::create_dir(&parent).unwrap();
        let target_path = parent.join("target.txt");
        let (action, call) = direct_file_change_fixture(
            &run_id,
            &conversation_id,
            &call_id,
            ("observed/target.txt", target_path.to_str().unwrap()),
            None,
            Some("frozen target\n"),
            AgentApprovalStatus::Required,
        );
        let record = seed_interrupted_manual_file_change(
            &storage,
            &run_id,
            &conversation_id,
            &assistant_message_id,
            action,
            &call,
        );
        match case {
            Case::Base => {}
            Case::Divergent => std::fs::write(&target_path, "different content\n").unwrap(),
            #[cfg(unix)]
            Case::Unreadable => {
                use std::os::unix::fs::symlink;
                let frozen_parent = canonical_fixture.join("observed-original");
                std::fs::rename(&parent, &frozen_parent).unwrap();
                let redirected_parent = canonical_fixture.join("redirected");
                std::fs::create_dir(&redirected_parent).unwrap();
                symlink(&redirected_parent, &parent).unwrap();
            }
        }

        assert_eq!(
            reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms())
                .unwrap(),
            1
        );
        let pending = storage
            .get_pending_agent_action(&record.storage_id)
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, "executing");
        assert_eq!(pending.target_status.as_deref(), Some("failed"));
        let audit = storage
            .get_agent_action_audit(&record.storage_id)
            .unwrap()
            .unwrap();
        assert_eq!(audit.status, "failed");
        assert_eq!(audit.decision_source.as_deref(), Some("manual"));
        let result: AgentFileChangeResult = serde_json::from_str(
            audit
                .file_change_result_json
                .as_deref()
                .expect("reconciliation persists the typed FileChange result"),
        )
        .unwrap();
        let tool_result: AgentToolResult = serde_json::from_str(
            audit
                .tool_result_json
                .as_deref()
                .expect("reconciliation persists the exact ToolResult"),
        )
        .unwrap();
        assert!(!tool_result.ok);
        match case {
            Case::Base => {
                assert_eq!(
                    result.status,
                    mycopilot_core::AgentFileChangeResultStatus::Failed
                );
                assert_eq!(
                    result.outcome,
                    mycopilot_core::AgentFileChangeOutcome::DefinitelyNotExecuted
                );
            }
            Case::Divergent => {
                assert_eq!(
                    result.status,
                    mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown
                );
                assert_eq!(
                    result.outcome,
                    mycopilot_core::AgentFileChangeOutcome::OutcomeUnknown
                );
            }
            #[cfg(unix)]
            Case::Unreadable => {
                assert_eq!(
                    result.status,
                    mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown
                );
                assert_eq!(
                    result.outcome,
                    mycopilot_core::AgentFileChangeOutcome::OutcomeUnknown
                );
            }
        }
        storage
            .reconcile_interrupted_pending_agent_actions(mycopilot_core::storage::now_ms())
            .unwrap();
        assert_eq!(
            storage
                .get_pending_agent_action(&record.storage_id)
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
    }
}

#[test]
fn startup_reconciles_published_manual_staged_file_changes_without_replaying_them() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let run_id = "run-manual-staged-crash";
    let conversation_id = "conversation-manual-staged-crash";
    let assistant_message_id = "assistant-manual-staged-crash";
    let call_id = "call-manual-staged-crash";
    let transaction_id = "transaction-manual-staged-crash";
    let target_content = "published staged content from apply_patch\n";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join("manual-staged.txt");
    let (file_change, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id,
        },
        ("manual-staged.txt", target_path.to_str().unwrap()),
        target_content,
        AgentApprovalStatus::Required,
    );
    let action = AgentProposedAction::FileChange {
        file_change: file_change.clone(),
    };

    std::fs::write(&target_path, target_content).unwrap();
    let metadata_before = std::fs::metadata(&target_path).unwrap();
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    let storage_id = pending_action_storage_id(run_id, call_id);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&storage_id),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    ));
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    storage
        .create_agent_file_change(staged_file_change_record(&file_change, "applying"))
        .unwrap();
    let record = PendingActionRecord {
        storage_id,
        snapshot: PendingAgentActionSnapshot {
            action_id: call_id.to_string(),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_call_id: Some(call_id.to_string()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action,
            created_at: 1,
            status: PendingActionStatus::Executing,
        },
        agent_input,
    };
    storage
        .store_pending_agent_action(pending_storage_record(&record, 2).unwrap())
        .unwrap();
    assert!(storage
        .insert_agent_action_audit_if_absent(action_audit_record(
            &record, None, "pending", None, None, None, None, None, None,
        ))
        .unwrap());

    assert_eq!(
        reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms()).unwrap(),
        1
    );
    let transaction = storage
        .get_agent_file_change(transaction_id)
        .unwrap()
        .unwrap();
    assert_eq!(transaction.status, "applied");
    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        target_content
    );
    let metadata_after = std::fs::metadata(&target_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(metadata_after.dev(), metadata_before.dev());
        assert_eq!(metadata_after.ino(), metadata_before.ino());
    }
    #[cfg(not(unix))]
    let _ = (metadata_before, metadata_after);

    let durable = storage
        .get_pending_agent_action(&record.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "executing");
    assert_eq!(durable.target_status.as_deref(), Some("completed"));
    let committed: AgentProposedAction = serde_json::from_str(&durable.action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = committed else {
        panic!("recovered Staged action must remain FileChange")
    };
    assert_eq!(file_change.execution.source_tool_name, "apply_patch");
    assert!(file_change.execution.receipt.is_some());

    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("published Staged FileChange must reconcile without replay");
    let results = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].call_id, call_id);
    assert_eq!(results[0].tool, "apply_patch");
    assert!(results[0].ok);
}

#[test]
fn startup_reconciles_published_automatic_direct_file_change_without_replaying_it() {
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
    // Construct the Host before seeding the synthetic interrupted turn so startup recovery does
    // not terminalize the fixture trace before its hidden Pending journal exists.
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-automatic-direct-crash";
    let conversation_id = "conversation-automatic-direct-crash";
    let assistant_message_id = "assistant-automatic-direct-crash";
    let call_id = "call-automatic-direct-crash";
    let target_content = "automatically published before the durable receipt\n";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join("automatic-recovered.txt");
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("automatic-recovered.txt", target_path.to_str().unwrap()),
        None,
        Some(target_content),
        AgentApprovalStatus::Approved,
    );
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    let storage_id = pending_action_storage_id(run_id, call_id);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&storage_id),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    ));
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    let mut hidden = service
        .prepare_auto_file_change_action_journal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    assert!(service
        .claim_auto_file_change_dispatch(&mut hidden)
        .unwrap());

    // Simulate a crash after filesystem publication but before the receipt/terminal audit.
    std::fs::write(&target_path, target_content).unwrap();
    let metadata_before = std::fs::metadata(&target_path).unwrap();
    let candidates = storage
        .list_interrupted_agent_actions_for_host_reconciliation()
        .unwrap();
    assert_eq!(candidates.len(), 1);
    let prepared: AgentProposedAction = serde_json::from_str(&candidates[0].action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: prepared,
    } = prepared
    else {
        panic!("automatic crash fixture must retain a FileChange");
    };
    prepared.execution.validate().unwrap();
    let resolved = mycopilot_core::file_change::FileChangePathPolicy::new(None, true)
        .resolve(&prepared.execution.canonical_target)
        .unwrap();
    let plan = mycopilot_core::file_change::FileChangePlan::from_direct_binding(
        &prepared.execution,
        prepared.inline_diff.as_ref().unwrap().patch.clone(),
    )
    .unwrap();
    assert_eq!(
        mycopilot_core::file_change::FileChangeCommitter
            .reconcile(&resolved, &plan, prepared.execution.delete_journal.as_ref())
            .unwrap(),
        mycopilot_core::file_change::FileChangeReconciliation::AlreadyApplied
    );

    assert_eq!(
        reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms()).unwrap(),
        1,
        "the automatic Direct startup pre-pass must recognize the already-published target"
    );
    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("published automatic Direct FileChange must reconcile from its target digest");

    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        target_content
    );
    let metadata_after = std::fs::metadata(&target_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(metadata_after.dev(), metadata_before.dev());
        assert_eq!(metadata_after.ino(), metadata_before.ino());
    }
    #[cfg(not(unix))]
    let _ = (metadata_before, metadata_after);

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let (status, action_json, tool_result_json): (String, String, Option<String>) = connection
        .query_row(
            "SELECT status, action_json, tool_result_json
             FROM agent_action_audit WHERE action_id = ?1",
            [pending_action_storage_id(run_id, call_id)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert!(tool_result_json.is_some());
    let committed: AgentProposedAction = serde_json::from_str(&action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = committed else {
        panic!("recovered automatic action must remain a FileChange");
    };
    assert_eq!(
        file_change.execution.transaction.status,
        mycopilot_core::file_change::FileChangeStatus::AlreadyApplied
    );
    assert!(file_change.execution.receipt.is_some());
}

#[test]
fn startup_reconciles_published_automatic_staged_file_changes_without_replaying_them() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    // Construct the Host before seeding the synthetic interrupted turn so startup recovery does
    // not terminalize the fixture trace before its hidden Pending journal exists.
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-automatic-staged-crash";
    let conversation_id = "conversation-automatic-staged-crash";
    let assistant_message_id = "assistant-automatic-staged-crash";
    let call_id = "call-automatic-staged-crash";
    let transaction_id = "transaction-automatic-staged-crash";
    let target_content = "automatically published from apply_patch\n";
    let relative_path = "automatic-staged.txt";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join(relative_path);
    let (file_change, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id,
        },
        (relative_path, target_path.to_str().unwrap()),
        target_content,
        AgentApprovalStatus::Approved,
    );
    let action = AgentProposedAction::FileChange {
        file_change: file_change.clone(),
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    let storage_id = pending_action_storage_id(run_id, call_id);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&storage_id),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    ));
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    storage
        .create_agent_file_change(staged_file_change_record(&file_change, "applying"))
        .unwrap();
    let mut hidden = service
        .prepare_auto_file_change_action_journal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    assert!(service
        .claim_auto_file_change_dispatch(&mut hidden)
        .unwrap());

    std::fs::write(&target_path, target_content).unwrap();
    let metadata_before = std::fs::metadata(&target_path).unwrap();
    assert_eq!(
        reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms()).unwrap(),
        1
    );
    let transaction = storage
        .get_agent_file_change(transaction_id)
        .unwrap()
        .unwrap();
    assert_eq!(transaction.status, "applied");
    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        target_content
    );
    let metadata_after = std::fs::metadata(&target_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(metadata_after.dev(), metadata_before.dev());
        assert_eq!(metadata_after.ino(), metadata_before.ino());
    }
    #[cfg(not(unix))]
    let _ = (metadata_before, metadata_after);
    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("published automatic Staged FileChange must reconcile without replay");
    let results = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].call_id, call_id);
    assert_eq!(results[0].tool, "apply_patch");
    assert!(results[0].ok);
}

#[test]
fn startup_finalizes_a_committed_automatic_direct_delete_before_terminal_audit() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    // Construct the Host before seeding the synthetic interrupted turn so startup recovery does
    // not terminalize the fixture trace before its hidden Pending journal exists.
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-automatic-delete-finalize-crash";
    let conversation_id = "conversation-automatic-delete-finalize-crash";
    let assistant_message_id = "assistant-automatic-delete-finalize-crash";
    let call_id = "call-automatic-delete-finalize-crash";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join("automatic-delete-finalize.txt");
    std::fs::write(&target_path, "private deleted contents\n").unwrap();
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        (
            "automatic-delete-finalize.txt",
            target_path.to_str().unwrap(),
        ),
        Some("private deleted contents\n"),
        None,
        AgentApprovalStatus::Approved,
    );
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    let storage_id = pending_action_storage_id(run_id, call_id);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&storage_id),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    ));
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    let mut hidden = service
        .prepare_auto_file_change_action_journal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    assert!(service
        .claim_auto_file_change_dispatch(&mut hidden)
        .unwrap());

    let mut committed_action = hidden.snapshot.action.clone();
    let AgentProposedAction::FileChange { file_change } = &mut committed_action else {
        unreachable!("fixture must produce a FileChange")
    };
    let target =
        mycopilot_core::file_change::FileChangePathPolicy::new(Some(fixture.path()), false)
            .resolve("automatic-delete-finalize.txt")
            .unwrap();
    let plan = mycopilot_core::file_change::FileChangePlan::from_direct_binding(
        &file_change.execution,
        file_change.inline_diff.as_ref().unwrap().patch.clone(),
    )
    .unwrap();
    let mut journal = file_change.execution.delete_journal.clone().unwrap();
    let commit = mycopilot_core::file_change::FileChangeCommitter
        .commit(
            &file_change.execution.transaction.id,
            &target,
            &plan,
            2,
            Some(&mut journal),
        )
        .unwrap();
    let tombstone_path = commit
        .delete_journal
        .as_ref()
        .unwrap()
        .tombstone_path
        .clone();
    *file_change.execution = file_change.execution.with_commit(&commit).unwrap();
    service
        .commit_pending_file_change_action(&mut hidden, committed_action)
        .unwrap();
    assert!(!target_path.exists());
    assert!(std::path::Path::new(&tombstone_path).exists());

    assert_eq!(
        reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms()).unwrap(),
        1
    );
    assert!(!target_path.exists());
    assert!(!std::path::Path::new(&tombstone_path).exists());

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let (status, action_json, tool_result_json): (String, String, Option<String>) = connection
        .query_row(
            "SELECT status, action_json, tool_result_json
             FROM agent_action_audit WHERE action_id = ?1",
            [pending_action_storage_id(run_id, call_id)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert!(tool_result_json.is_some());
    let finalized: AgentProposedAction = serde_json::from_str(&action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = finalized else {
        unreachable!("finalized action remains a FileChange")
    };
    assert_eq!(
        file_change.execution.delete_journal.as_ref().unwrap().state,
        mycopilot_core::file_change::FileChangeDeleteJournalState::Finalized
    );
    assert!(file_change.execution.receipt.is_some());
}

#[test]
fn file_change_pending_checkpoint_requires_exact_canonical_pending_action_id() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-file-change-checkpoint-id";
    let conversation_id = "conversation-file-change-checkpoint-id";
    let assistant_message_id = "assistant-file-change-checkpoint-id";
    let call_id = "call-file-change-checkpoint-id";
    let target = fixture.path().join("checkpoint-id.txt");
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("checkpoint-id.txt", target.to_str().unwrap()),
        None,
        Some("checkpoint identity\n"),
        AgentApprovalStatus::Required,
    );
    let mut base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut base_input);
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    base_input.context = Some(run_context.clone());
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    let canonical_id = pending_action_storage_id(run_id, call_id);

    for pending_action_id in [None, Some("v2:tampered")].into_iter() {
        let mut input = base_input.clone();
        let mut checkpoint = test_pending_resume_checkpoint_for_call(
            &storage,
            run_id,
            pending_action_id,
            &call,
            AgentToolIdentity::Builtin {
                tool_name: "apply_patch".to_string(),
            },
        );
        checkpoint.run_context = Some(run_context.clone());
        input.resume_checkpoint = Some(checkpoint);
        freeze_test_pending_provider_configuration(&storage, &mut input);
        let error = service
            .store_pending_action(
                run_id,
                conversation_id,
                assistant_message_id,
                action.clone(),
                input,
            )
            .unwrap_err();
        assert_eq!(
            error,
            "Pending action frozen Tool Call identity is inconsistent."
        );
        assert!(storage.list_pending_agent_actions().unwrap().is_empty());
    }

    let mut exact_input = base_input;
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&canonical_id),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    );
    checkpoint.run_context = Some(run_context);
    exact_input.resume_checkpoint = Some(checkpoint);
    freeze_test_pending_provider_configuration(&storage, &mut exact_input);
    assert!(service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            exact_input,
        )
        .unwrap());
    assert_eq!(
        storage.list_pending_agent_actions().unwrap()[0].action_id,
        canonical_id
    );
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

#[derive(Clone)]
struct AutoActivationTestProvider {
    manifest: mycopilot_core::BuiltinCapabilityManifest,
    grant: Arc<Mutex<Option<mycopilot_core::CapabilityGrant>>>,
    approvals: Arc<std::sync::atomic::AtomicUsize>,
    revocations: Arc<std::sync::atomic::AtomicUsize>,
    cancel_on_approve: Arc<Mutex<Option<AgentCancellationToken>>>,
}

impl mycopilot_core::BuiltinCapabilityProvider for AutoActivationTestProvider {
    fn manifests(&self) -> AgentResult<Vec<mycopilot_core::BuiltinCapabilityManifest>> {
        Ok(vec![self.manifest.clone()])
    }

    fn policy(
        &self,
        _capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<mycopilot_core::BuiltinCapabilityPolicy> {
        Ok(mycopilot_core::BuiltinCapabilityPolicy {
            user_allowed: true,
            revision: 1,
        })
    }

    fn grant(
        &self,
        _run_id: &str,
        _capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<Option<mycopilot_core::CapabilityGrant>> {
        Ok(self.grant.lock().unwrap().clone())
    }

    fn approve_activation(
        &self,
        approval: &mycopilot_core::AgentBuiltinCapabilityActivationApproval,
    ) -> AgentResult<mycopilot_core::CapabilityGrant> {
        self.approvals
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let grant = mycopilot_core::CapabilityGrant {
            run_id: approval.run_id.clone(),
            capability_id: mycopilot_core::BuiltinCapabilityId::parse(
                approval.capability_id.clone(),
            )?,
            activation_id: mycopilot_core::CapabilityActivationId::parse(
                approval.activation_id.clone(),
            )?,
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
                .saturating_add(mycopilot_core::BUILTIN_CAPABILITY_GRANT_TTL_SECONDS),
        };
        *self.grant.lock().unwrap() = Some(grant.clone());
        if let Some(cancellation) = self.cancel_on_approve.lock().unwrap().take() {
            cancellation.cancel();
        }
        Ok(grant)
    }

    fn revoke_activation(
        &self,
        _activation_id: &mycopilot_core::CapabilityActivationId,
        _action_id: &str,
    ) -> AgentResult<()> {
        self.revocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.grant.lock().unwrap() = None;
        Ok(())
    }

    fn revoke_grants(
        &self,
        _capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<()> {
        *self.grant.lock().unwrap() = None;
        Ok(())
    }

    fn invoke_authorized<'a>(
        &'a self,
        _invocation: mycopilot_core::BuiltinCapabilityInvocation,
        _expected_grant: mycopilot_core::CapabilityGrant,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::BuiltinCapabilityFuture<'a, Value> {
        Box::pin(async { Err(AgentError::new("not used by activation test")) })
    }
}

fn auto_activation_test_provider() -> AutoActivationTestProvider {
    let manifest = mycopilot_core::BuiltinCapabilityManifest::new(
        mycopilot_core::BuiltinCapabilityDescriptor {
            id: mycopilot_core::BuiltinCapabilityId::parse("browser_automation").unwrap(),
            display_name: "Browser automation".to_string(),
            description: "Control the reviewed browser".to_string(),
        },
        "builtin.browser_automation.mcp",
        "fixture-v1",
        vec![mycopilot_core::BuiltinCapabilityToolDescriptor::new(
            "browser_snapshot",
            "browser_snapshot",
            "Read the current page",
            json!({"type":"object", "additionalProperties": false}),
            mycopilot_core::AgentToolSafety::ReadOnly,
            false,
        )
        .unwrap()],
    )
    .unwrap();
    AutoActivationTestProvider {
        manifest,
        grant: Arc::new(Mutex::new(None)),
        approvals: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        revocations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        cancel_on_approve: Arc::new(Mutex::new(None)),
    }
}

fn auto_activation_test_input() -> AgentChatInput {
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.context = Some(mycopilot_core::AgentRunContext {
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: mycopilot_core::AgentPermissions {
            builtin_execution: mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove,
            ..mycopilot_core::AgentPermissions::default()
        },
        collaboration_identity: None,
    });
    input
}

fn auto_activation_test_action(
    provider: &AutoActivationTestProvider,
    run_id: &str,
) -> AgentProposedAction {
    let now = u64::try_from(mycopilot_core::storage::now_ms()).unwrap() / 1_000;
    AgentProposedAction::BuiltinCapabilityActivation {
        approval: Box::new(mycopilot_core::AgentBuiltinCapabilityActivationApproval {
            action_id: uuid::Uuid::new_v4().to_string(),
            activation_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            call_id: "auto-activation-call".to_string(),
            capability_id: "browser_automation".to_string(),
            display_name: "Browser automation".to_string(),
            reason: "Open the reviewed browser".to_string(),
            manifest_digest: provider.manifest.manifest_digest.clone(),
            policy_revision: 1,
            created_at: now,
            expires_at: now.saturating_add(900),
            approval_status: AgentApprovalStatus::Approved,
        }),
    }
}

#[derive(Clone)]
struct AutoSensitiveTestProvider {
    manifest: mycopilot_core::BuiltinCapabilityManifest,
    capability_grant: mycopilot_core::CapabilityGrant,
    storage: Arc<StorageService>,
    invocations: Arc<std::sync::atomic::AtomicUsize>,
    revocations: Arc<std::sync::atomic::AtomicUsize>,
}

impl mycopilot_core::BuiltinCapabilityProvider for AutoSensitiveTestProvider {
    fn manifests(&self) -> AgentResult<Vec<mycopilot_core::BuiltinCapabilityManifest>> {
        Ok(vec![self.manifest.clone()])
    }

    fn policy(
        &self,
        _capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<mycopilot_core::BuiltinCapabilityPolicy> {
        Ok(mycopilot_core::BuiltinCapabilityPolicy {
            user_allowed: true,
            revision: 1,
        })
    }

    fn grant(
        &self,
        run_id: &str,
        capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<Option<mycopilot_core::CapabilityGrant>> {
        Ok((self.capability_grant.run_id == run_id
            && self.capability_grant.capability_id == *capability_id)
            .then(|| self.capability_grant.clone()))
    }

    fn approve_activation(
        &self,
        _approval: &mycopilot_core::AgentBuiltinCapabilityActivationApproval,
    ) -> AgentResult<mycopilot_core::CapabilityGrant> {
        Err(AgentError::new("not used by sensitive auto test"))
    }

    fn revoke_activation(
        &self,
        _activation_id: &mycopilot_core::CapabilityActivationId,
        _action_id: &str,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn revoke_grants(
        &self,
        _capability_id: &mycopilot_core::BuiltinCapabilityId,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn approve_builtin_mcp_tool(
        &self,
        approval: &mycopilot_core::AgentBuiltinMcpToolApproval,
    ) -> AgentResult<mycopilot_core::BuiltinMcpToolGrant> {
        let identity = &approval.identity;
        Ok(mycopilot_core::BuiltinMcpToolGrant {
            grant_id: uuid::Uuid::new_v4().to_string(),
            approval_id: identity.approval_id.clone(),
            run_id: identity.run_id.clone(),
            call_id: identity.call_id.clone(),
            capability_id: mycopilot_core::BuiltinCapabilityId::parse(
                identity.capability_id.clone(),
            )?,
            capability_activation_id: mycopilot_core::CapabilityActivationId::parse(
                identity.capability_activation_id.clone(),
            )?,
            managed_mcp_id: identity.managed_mcp_id.clone(),
            package_name: identity.package_name.clone(),
            package_version: identity.package_version.clone(),
            upstream_catalog_digest: identity.upstream_catalog_digest.clone(),
            manifest_digest: identity.manifest_digest.clone(),
            policy_digest: identity.policy_digest.clone(),
            policy_revision: identity.policy_revision,
            tool_id: identity.tool_id.clone(),
            raw_name: identity.raw_name.clone(),
            model_name: identity.model_name.clone(),
            upstream_schema_digest: identity.upstream_schema_digest.clone(),
            host_overlay_digest: identity.host_overlay_digest.clone(),
            host_input_schema_digest: identity.host_input_schema_digest.clone(),
            arguments_digest: identity.arguments_digest.clone(),
            resource_scope_digest: identity.resource_scope_digest.clone(),
            target_binding_id: None,
            target_binding_digest: None,
            origin: identity.origin.clone(),
            risk_kinds: approval.risk_kinds.clone(),
            created_at: approval.created_at,
            expires_at: approval.expires_at,
        })
    }

    fn revoke_builtin_mcp_tool_grant(
        &self,
        _grant_id: &str,
        _approval_id: &str,
    ) -> AgentResult<()> {
        self.revocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn invoke_approved_builtin_mcp_tool<'a>(
        &'a self,
        approval: mycopilot_core::AgentBuiltinMcpToolApproval,
        _grant: mycopilot_core::BuiltinMcpToolGrant,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::BuiltinCapabilityFuture<'a, Value> {
        Box::pin(async move {
            let storage_id =
                pending_action_storage_id(&approval.identity.run_id, &approval.identity.action_id);
            let record = self
                .storage
                .get_pending_agent_action(&storage_id)?
                .ok_or_else(|| AgentError::new("missing durable sensitive auto journal"))?;
            if record.status != "executing" {
                return Err(AgentError::new(
                    "sensitive invocation ran before the durable executing claim",
                ));
            }
            self.invocations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(json!({"status": "ok"}))
        })
    }

    fn invoke_authorized<'a>(
        &'a self,
        _invocation: mycopilot_core::BuiltinCapabilityInvocation,
        _expected_grant: mycopilot_core::CapabilityGrant,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::BuiltinCapabilityFuture<'a, Value> {
        Box::pin(async { Err(AgentError::new("not used by sensitive auto test")) })
    }
}

fn auto_sensitive_test_fixture(
    storage: Arc<StorageService>,
    run_id: &str,
    call_id: &str,
) -> (
    mycopilot_core::BuiltinCapabilityRuntime,
    AutoSensitiveTestProvider,
    mycopilot_core::AgentBuiltinMcpToolApproval,
    AgentChatInput,
) {
    let descriptor = mycopilot_core::BuiltinCapabilityToolDescriptor::new(
        "browser_evaluate",
        "browser_evaluate",
        "Evaluate a reviewed script",
        json!({
            "type": "object",
            "properties": {"function": {"type": "string"}},
            "required": ["function"],
            "additionalProperties": false
        }),
        mycopilot_core::AgentToolSafety::RequiresApproval,
        false,
    )
    .unwrap()
    .with_builtin_approval_policy(
        mycopilot_core::BuiltinMcpToolApprovalMode::Always,
        vec![mycopilot_core::BuiltinMcpToolRiskKind::PageScriptExecution],
    )
    .unwrap();
    let manifest = mycopilot_core::BuiltinCapabilityManifest::new(
        mycopilot_core::BuiltinCapabilityDescriptor {
            id: mycopilot_core::BuiltinCapabilityId::parse("browser_automation").unwrap(),
            display_name: "Browser automation".to_string(),
            description: "Control the reviewed browser".to_string(),
        },
        "builtin.browser_automation.mcp",
        "fixture-v1",
        vec![descriptor.clone()],
    )
    .unwrap();
    let now = u64::try_from(mycopilot_core::storage::now_ms()).unwrap() / 1_000;
    let activation_id = mycopilot_core::CapabilityActivationId::generate();
    let capability_grant = mycopilot_core::CapabilityGrant {
        run_id: run_id.to_string(),
        capability_id: manifest.descriptor.id.clone(),
        activation_id: activation_id.clone(),
        manifest_digest: manifest.manifest_digest.clone(),
        upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
        provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
        policy_revision: 1,
        created_at: now,
        expires_at: now.saturating_add(mycopilot_core::BUILTIN_CAPABILITY_GRANT_TTL_SECONDS),
    };
    let provider = AutoSensitiveTestProvider {
        manifest: manifest.clone(),
        capability_grant,
        storage: Arc::clone(&storage),
        invocations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        revocations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let runtime =
        mycopilot_core::BuiltinCapabilityRuntime::new(Arc::new(provider.clone())).unwrap();
    let action_id = uuid::Uuid::new_v4().to_string();
    let approval = mycopilot_core::AgentBuiltinMcpToolApproval {
        schema_version: mycopilot_core::BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION,
        identity: mycopilot_core::BuiltinMcpToolApprovalIdentity {
            action_id: action_id.clone(),
            approval_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            call_id: call_id.to_string(),
            capability_id: manifest.descriptor.id.as_str().to_string(),
            capability_activation_id: activation_id.as_str().to_string(),
            managed_mcp_id: manifest.managed_mcp_id.clone(),
            package_name: manifest.provider_contract.package_name.clone(),
            package_version: manifest.provider_contract.package_version.clone(),
            upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            policy_digest: manifest.provider_contract.policy_digest.clone(),
            policy_revision: 1,
            tool_id: descriptor.tool_id.clone(),
            raw_name: descriptor.raw_name.clone(),
            model_name: descriptor.model_name.clone(),
            upstream_schema_digest: descriptor.upstream_schema_digest.clone(),
            host_overlay_digest: descriptor.host_overlay_digest.clone(),
            host_input_schema_digest: descriptor.schema_digest.clone(),
            arguments_digest: format!("sha256:{}", "7".repeat(64)),
            resource_scope_digest: format!("sha256:{}", "8".repeat(64)),
            origin: Some("https://mail.example.test".to_string()),
        },
        capability_display_name: manifest.descriptor.display_name.clone(),
        tool_display_name: descriptor.model_name.clone(),
        call_reason: "Run the reviewed page script.".to_string(),
        operation_category: "page_script_execution".to_string(),
        resource_summary: mycopilot_core::BuiltinMcpToolResourceSummary {
            scope: "managed_surface".to_string(),
            display_name: "Current managed page".to_string(),
            file_basenames: Vec::new(),
            origin: Some("https://mail.example.test".to_string()),
        },
        risk_kinds: vec![mycopilot_core::BuiltinMcpToolRiskKind::PageScriptExecution],
        created_at: now,
        expires_at: now.saturating_add(900),
        approval_status: AgentApprovalStatus::Approved,
    };
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: descriptor.model_name.clone(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Approved,
        reason: Some(approval.call_reason.clone()),
    };
    let provenance = AgentToolIdentity::BuiltinCapability {
        capability_id: approval.identity.capability_id.clone().into(),
        managed_mcp_id: approval.identity.managed_mcp_id.clone().into(),
        package_name: approval.identity.package_name.clone().into(),
        package_version: approval.identity.package_version.clone().into(),
        upstream_catalog_digest: approval.identity.upstream_catalog_digest.clone().into(),
        policy_digest: approval.identity.policy_digest.clone().into(),
        manifest_digest: approval.identity.manifest_digest.clone().into(),
        tool_id: approval.identity.tool_id.clone().into(),
        raw_name: approval.identity.raw_name.clone().into(),
        model_name: approval.identity.model_name.clone().into(),
        upstream_schema_digest: approval.identity.upstream_schema_digest.clone().into(),
        host_overlay_digest: approval.identity.host_overlay_digest.clone().into(),
        host_input_schema_digest: approval.identity.host_input_schema_digest.clone().into(),
    };
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": {"imageInput": false},
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut input);
    let run_context = mycopilot_core::AgentRunContext {
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: mycopilot_core::AgentPermissions {
            builtin_execution: mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove,
            ..mycopilot_core::AgentPermissions::default()
        },
        collaboration_identity: None,
    };
    input.context = Some(run_context.clone());
    input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&action_id),
        &call,
        provenance,
    ));
    input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    (runtime, provider, approval, input)
}

#[test]
fn auto_activation_has_durable_non_replayable_receipt_and_cancellation_revokes_grant() {
    for cancel_during_approval in [false, true] {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("auto-activation.sqlite")).unwrap());
        let provider = auto_activation_test_provider();
        let runtime =
            mycopilot_core::BuiltinCapabilityRuntime::new(Arc::new(provider.clone())).unwrap();
        let service = AgentService::new(Arc::clone(&storage)).with_builtin_capabilities(runtime);
        let run_id = if cancel_during_approval {
            "auto-activation-cancel-run"
        } else {
            "auto-activation-success-run"
        };
        let action = auto_activation_test_action(&provider, run_id);
        let input = auto_activation_test_input();
        let cancellation = AgentCancellationToken::new();
        if cancel_during_approval {
            *provider.cancel_on_approve.lock().unwrap() = Some(cancellation.clone());
        }
        let context =
            AutoApprovedActionContext::new(input.clone(), run_id.to_string(), None, None, None);
        let result = service
            .execute_auto_approved_action(context, action.clone(), cancellation)
            .unwrap();
        assert_eq!(
            result.ok, !cancel_during_approval,
            "cancellation after grant minting must be reflected in the durable result"
        );
        assert_eq!(
            provider.approvals.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            provider.grant.lock().unwrap().is_some(),
            !cancel_during_approval
        );
        assert_eq!(
            provider
                .revocations
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(cancel_during_approval)
        );

        let replay = service.execute_auto_approved_action(
            AutoApprovedActionContext::new(input, run_id.to_string(), None, None, None),
            action,
            AgentCancellationToken::new(),
        );
        assert!(
            replay.is_err(),
            "a durable activation receipt must never replay"
        );
        assert_eq!(
            provider.approvals.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }
}

#[test]
fn builtin_capability_pending_binding_requires_exact_action_and_call_ids() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open(&fixture.path().join("builtin-capability-binding.sqlite")).unwrap(),
    );
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let run_id = "builtin-capability-binding-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let activation_id = uuid::Uuid::new_v4().to_string();
    let call = AgentToolCall {
        id: "builtin-capability-call".to_string(),
        tool: "activate_capability".to_string(),
        args: json!({
            "capability": "browser_automation",
            "reason": "Inspect the task page"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::BuiltinCapabilityActivation {
        approval: Box::new(mycopilot_core::AgentBuiltinCapabilityActivationApproval {
            action_id: action_id.clone(),
            activation_id,
            run_id: run_id.to_string(),
            call_id: call.id.clone(),
            capability_id: "browser_automation".to_string(),
            display_name: "Browser automation".to_string(),
            reason: "Inspect the task page".to_string(),
            manifest_digest: format!("sha256:{}", "1".repeat(64)),
            policy_revision: 1,
            created_at: 1,
            expires_at: 901,
            approval_status: AgentApprovalStatus::Required,
        }),
    };
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut input);
    input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&action_id),
        &call,
        AgentToolIdentity::RuntimeExtension {
            extension_id: "builtin.capabilities".to_string(),
            tool_name: "activate_capability".to_string(),
        },
    ));

    assert!(pending_action_binding_matches(
        run_id, None, &action, &input
    ));

    {
        let checkpoint_call = input
            .resume_checkpoint
            .as_mut()
            .unwrap()
            .context_items
            .iter_mut()
            .flat_map(|item| item.tool_calls.iter_mut())
            .find(|candidate| candidate.id == call.id)
            .unwrap();
        checkpoint_call.args["reason"] = json!("A different model-authored reason");
    }
    assert!(!pending_action_binding_matches(
        run_id, None, &action, &input
    ));
    {
        let checkpoint_call = input
            .resume_checkpoint
            .as_mut()
            .unwrap()
            .context_items
            .iter_mut()
            .flat_map(|item| item.tool_calls.iter_mut())
            .find(|candidate| candidate.id == call.id)
            .unwrap();
        checkpoint_call.args = call.args.clone();
        checkpoint_call.args["capability"] = json!("another_capability");
    }
    assert!(!pending_action_binding_matches(
        run_id, None, &action, &input
    ));
    {
        let checkpoint_call = input
            .resume_checkpoint
            .as_mut()
            .unwrap()
            .context_items
            .iter_mut()
            .flat_map(|item| item.tool_calls.iter_mut())
            .find(|candidate| candidate.id == call.id)
            .unwrap();
        checkpoint_call.args = call.args.clone();
        checkpoint_call.args["unexpected"] = json!(true);
    }
    assert!(!pending_action_binding_matches(
        run_id, None, &action, &input
    ));
    input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .context_items
        .iter_mut()
        .flat_map(|item| item.tool_calls.iter_mut())
        .find(|candidate| candidate.id == call.id)
        .unwrap()
        .args = call.args.clone();
    assert!(pending_action_binding_matches(
        run_id, None, &action, &input
    ));

    input.resume_checkpoint.as_mut().unwrap().pending_action_id =
        Some(uuid::Uuid::new_v4().to_string());
    assert!(!pending_action_binding_matches(
        run_id, None, &action, &input
    ));

    input.resume_checkpoint.as_mut().unwrap().pending_action_id = Some(action_id.clone());
    let mut drifted_action = action.clone();
    let AgentProposedAction::BuiltinCapabilityActivation { approval } = &mut drifted_action else {
        unreachable!();
    };
    approval.run_id = "different-run".to_string();
    assert!(!pending_action_binding_matches(
        run_id,
        None,
        &drifted_action,
        &input
    ));

    let storage_id = pending_action_storage_id(run_id, &action_id);
    storage
        .upsert_agent_action_audit(AgentActionAuditRecord {
            action_id: storage_id.clone(),
            run_id: "conflicting-audit-owner".to_string(),
            conversation_id: None,
            assistant_message_id: None,
            action_type: "builtin_capability_activation".to_string(),
            tool_name: "activate_capability".to_string(),
            decision: None,
            status: "pending".to_string(),
            action_json: "{}".to_string(),
            file_change_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 1,
            decided_at: None,
            completed_at: None,
            effective_permissions_json: None,
            path_scope: None,
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("manual_pending".to_string()),
        })
        .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    let failed = service.store_pending_action(
        run_id,
        "builtin-capability-binding-conversation",
        "builtin-capability-binding-assistant",
        action.clone(),
        input.clone(),
    );
    assert!(
        failed.is_err(),
        "a conflicting second write must abort publication"
    );
    assert!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .is_none(),
        "the pending insert must roll back with the conflicting audit"
    );
    rusqlite::Connection::open(fixture.path().join("builtin-capability-binding.sqlite"))
        .unwrap()
        .execute(
            "DELETE FROM agent_action_audit WHERE action_id = ?1",
            [&storage_id],
        )
        .unwrap();

    assert!(service
        .store_pending_action(
            run_id,
            "builtin-capability-binding-conversation",
            "builtin-capability-binding-assistant",
            action,
            input,
        )
        .unwrap());
    drop(service);

    let restarted = AgentService::new(Arc::clone(&storage));
    let pending = restarted.list_pending_actions();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].action_id, action_id);
    assert_eq!(pending[0].tool_call_id.as_deref(), Some(call.id.as_str()));
    assert_eq!(pending[0].action_type, "builtin_capability_activation");
    let persisted_pair: (i64, i64) =
        rusqlite::Connection::open(fixture.path().join("builtin-capability-binding.sqlite"))
            .unwrap()
            .query_row(
                "SELECT
             (SELECT COUNT(*) FROM agent_pending_actions WHERE action_id = ?1),
             (SELECT COUNT(*) FROM agent_action_audit WHERE action_id = ?1 AND status = 'pending')",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(persisted_pair, (1, 1));
}

#[tokio::test]
async fn builtin_capability_approval_waits_past_its_proposal_window_and_can_still_be_approved() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("builtin-capability-expiry.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let run_id = "builtin-capability-expiry-run";
    let conversation_id = "builtin-capability-expiry-conversation";
    let assistant_message_id = "builtin-capability-expiry-assistant";
    let action_id = uuid::Uuid::new_v4().to_string();
    let expiry_now_ms = mycopilot_core::storage::now_ms();
    let expiry_now_seconds = u64::try_from(expiry_now_ms).unwrap() / 1_000;
    let provider = auto_activation_test_provider();
    let runtime =
        mycopilot_core::BuiltinCapabilityRuntime::new(Arc::new(provider.clone())).unwrap();
    let call = AgentToolCall {
        id: "builtin-capability-expiry-call".to_string(),
        tool: "activate_capability".to_string(),
        args: json!({
            "capability": "browser_automation",
            "reason": "Inspect the task page"
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::BuiltinCapabilityActivation {
        approval: Box::new(mycopilot_core::AgentBuiltinCapabilityActivationApproval {
            action_id: action_id.clone(),
            activation_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            call_id: call.id.clone(),
            capability_id: "browser_automation".to_string(),
            display_name: "Browser automation".to_string(),
            reason: "Inspect the task page".to_string(),
            manifest_digest: provider.manifest.manifest_digest.clone(),
            policy_revision: 1,
            created_at: expiry_now_seconds.saturating_sub(901),
            expires_at: expiry_now_seconds.saturating_sub(1),
            approval_status: AgentApprovalStatus::Required,
        }),
    };
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut input);
    input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&action_id),
        &call,
        AgentToolIdentity::RuntimeExtension {
            extension_id: "builtin.capabilities".to_string(),
            tool_name: "activate_capability".to_string(),
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
    input.context = Some(run_context.clone());
    input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    let service = AgentService::new(Arc::clone(&storage))
        .with_builtin_capabilities(runtime)
        .with_mcp_approval_clock(move || expiry_now_ms);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::RuntimeExtension {
            extension_id: "builtin.capabilities".to_string(),
            tool_name: "activate_capability".to_string(),
        },
        expiry_now_ms.saturating_sub(1_000),
    );

    assert!(service
        .store_pending_action(run_id, conversation_id, assistant_message_id, action, input,)
        .unwrap());
    assert_eq!(service.list_pending_actions().len(), 1);
    let pre_expiry_trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(pre_expiry_trace.run_id, run_id);
    assert_eq!(pre_expiry_trace.conversation_id, conversation_id);
    assert_eq!(pre_expiry_trace.assistant_message_id, assistant_message_id);
    assert_eq!(
        pre_expiry_trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    assert_eq!(
        service
            .reconcile_expired_builtin_capability_approvals()
            .unwrap(),
        0
    );
    assert_eq!(service.list_pending_actions().len(), 1);
    assert_eq!(
        service
            .reconcile_expired_builtin_capability_approvals()
            .unwrap(),
        0,
        "the periodic reconciler must leave human approval pending"
    );

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let pending: (String, String, String) = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status, action_json, agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(pending.0, "pending");
    assert_ne!(pending.1, "{}");
    assert_ne!(pending.2, "{}");
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        0,
        "waiting past the proposal window must not synthesize a failure result"
    );

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let output = service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
        .unwrap();
    assert_eq!(output.status, "approved");
    assert_eq!(
        provider.approvals.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let grant = provider.grant.lock().unwrap().clone().unwrap();
    assert_eq!(grant.created_at, expiry_now_seconds);
    assert!(grant.created_at > expiry_now_seconds.saturating_sub(1));
}

fn store_builtin_sensitive_test_pending(
    service: &AgentService,
    storage: &StorageService,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    call_id: &str,
) -> (String, AgentToolCall) {
    let now_ms = mycopilot_core::storage::now_ms();
    let now_seconds = u64::try_from(now_ms).unwrap() / 1_000;
    let action_id = uuid::Uuid::new_v4().to_string();
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: "browser_evaluate".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: Some("Run the reviewed page script.".to_string()),
    };
    let approval = mycopilot_core::AgentBuiltinMcpToolApproval {
        schema_version: mycopilot_core::BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION,
        identity: mycopilot_core::BuiltinMcpToolApprovalIdentity {
            action_id: action_id.clone(),
            approval_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            call_id: call.id.clone(),
            capability_id: "browser_automation".to_string(),
            capability_activation_id: uuid::Uuid::new_v4().to_string(),
            managed_mcp_id: "builtin.browser_automation.mcp".to_string(),
            package_name: "@playwright/mcp".to_string(),
            package_version: "0.0.79".to_string(),
            upstream_catalog_digest: format!("sha256:{}", "1".repeat(64)),
            manifest_digest: format!("sha256:{}", "2".repeat(64)),
            policy_digest: format!("sha256:{}", "3".repeat(64)),
            policy_revision: 1,
            tool_id: "browser_evaluate".to_string(),
            raw_name: "browser_evaluate".to_string(),
            model_name: "browser_evaluate".to_string(),
            upstream_schema_digest: format!("sha256:{}", "4".repeat(64)),
            host_overlay_digest: format!("sha256:{}", "5".repeat(64)),
            host_input_schema_digest: format!("sha256:{}", "6".repeat(64)),
            arguments_digest: format!("sha256:{}", "7".repeat(64)),
            resource_scope_digest: format!("sha256:{}", "8".repeat(64)),
            origin: Some("https://mail.example.test".to_string()),
        },
        capability_display_name: "Browser automation".to_string(),
        tool_display_name: "Evaluate page script".to_string(),
        call_reason: "Run the reviewed page script.".to_string(),
        operation_category: "page_script_execution".to_string(),
        resource_summary: mycopilot_core::BuiltinMcpToolResourceSummary {
            scope: "page_script_execution".to_string(),
            display_name: "Current managed page".to_string(),
            file_basenames: Vec::new(),
            origin: Some("https://mail.example.test".to_string()),
        },
        risk_kinds: vec![mycopilot_core::BuiltinMcpToolRiskKind::PageScriptExecution],
        created_at: now_seconds,
        expires_at: now_seconds.saturating_add(900),
        approval_status: AgentApprovalStatus::Required,
    };
    let action = AgentProposedAction::BuiltinMcpToolApproval {
        approval: Box::new(approval),
    };
    let provenance = AgentToolIdentity::BuiltinCapability {
        capability_id: "browser_automation".into(),
        managed_mcp_id: "builtin.browser_automation.mcp".into(),
        package_name: "@playwright/mcp".into(),
        package_version: "0.0.79".into(),
        upstream_catalog_digest: format!("sha256:{}", "1".repeat(64)).into(),
        policy_digest: format!("sha256:{}", "3".repeat(64)).into(),
        manifest_digest: format!("sha256:{}", "2".repeat(64)).into(),
        tool_id: "browser_evaluate".into(),
        raw_name: "browser_evaluate".into(),
        model_name: "browser_evaluate".into(),
        upstream_schema_digest: format!("sha256:{}", "4".repeat(64)).into(),
        host_overlay_digest: format!("sha256:{}", "5".repeat(64)).into(),
        host_input_schema_digest: format!("sha256:{}", "6".repeat(64)).into(),
    };
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "http://127.0.0.1:9/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    let run_context = mycopilot_core::AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: mycopilot_core::AgentPermissions::default(),
        collaboration_identity: None,
    };
    input.context = Some(run_context.clone());
    freeze_test_pending_provider_configuration(storage, &mut input);
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        storage,
        run_id,
        Some(&action_id),
        &call,
        provenance.clone(),
    );
    checkpoint.run_context = Some(run_context);
    input.resume_checkpoint = Some(checkpoint);
    seed_durable_pending_owner(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        provenance,
        now_ms,
    );
    assert!(service
        .store_pending_action(run_id, conversation_id, assistant_message_id, action, input,)
        .unwrap());
    (action_id, call)
}

#[test]
fn automatic_builtin_sensitive_journal_is_durable_cancelable_and_non_replayable() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "auto-builtin-sensitive-journal-run";
    let (manual_action_id, _) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        "auto-builtin-sensitive-conversation",
        "auto-builtin-sensitive-assistant",
        "auto-builtin-sensitive-call",
    );
    let manual_storage_id = pending_action_storage_id(run_id, &manual_action_id);
    let template = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&manual_storage_id)
        .cloned()
        .unwrap();

    for (index, outcome) in [
        McpAutoActionJournalTerminalOutcome::Completed,
        McpAutoActionJournalTerminalOutcome::Cancelled,
    ]
    .into_iter()
    .enumerate()
    {
        let action_id = uuid::Uuid::new_v4().to_string();
        let mut action = template.snapshot.action.clone();
        let AgentProposedAction::BuiltinMcpToolApproval { approval } = &mut action else {
            unreachable!("fixture always creates a built-in MCP approval")
        };
        approval.identity.action_id = action_id.clone();
        approval.approval_status = AgentApprovalStatus::Approved;

        let mut input = template.agent_input.clone();
        let run_context = mycopilot_core::AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: mycopilot_core::AgentPermissions {
                builtin_execution: mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove,
                ..mycopilot_core::AgentPermissions::default()
            },
            collaboration_identity: None,
        };
        input.context = Some(run_context.clone());
        let checkpoint = input.resume_checkpoint.as_mut().unwrap();
        checkpoint.run_context = Some(run_context);
        checkpoint.pending_action_id = Some(action_id.clone());
        assert!(
            !pending_action_binding_matches(run_id, None, &action, &input),
            "the manual pending-action boundary must never accept an auto-approved action"
        );

        let mut journal = service
            .prepare_auto_mcp_action_journal(run_id, None, None, action.clone(), input.clone())
            .unwrap();
        assert_eq!(journal.snapshot.status, PendingActionStatus::Approved);
        if outcome == McpAutoActionJournalTerminalOutcome::Completed {
            service.claim_auto_mcp_dispatch(&mut journal).unwrap();
            assert_eq!(journal.snapshot.status, PendingActionStatus::Executing);
        }
        service
            .settle_auto_mcp_action_journal(&journal, outcome, None)
            .unwrap();

        let durable = storage
            .get_pending_agent_action(&pending_action_storage_id(run_id, &action_id))
            .unwrap()
            .unwrap();
        assert_eq!(
            durable.status,
            if outcome == McpAutoActionJournalTerminalOutcome::Completed {
                "completed"
            } else {
                "cancelled"
            },
            "case {index} must have an exact durable terminal state"
        );
        assert_eq!(durable.action_json, "{}");
        assert_eq!(durable.agent_input_json, "{}");
        assert!(service
            .prepare_auto_mcp_action_journal(run_id, None, None, action, input)
            .is_err());
    }
}

#[tokio::test]
async fn automatic_builtin_sensitive_execution_claims_before_invoke_and_never_replays() {
    for cancelled in [false, true] {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        save_test_pending_provider(
            &storage,
            "test-model",
            "https://example.test/v1/chat/completions",
            "test-token",
            "disabled",
            "",
        );
        let run_id = if cancelled {
            "auto-sensitive-cancel-run"
        } else {
            "auto-sensitive-success-run"
        };
        let (runtime, provider, approval, input) =
            auto_sensitive_test_fixture(Arc::clone(&storage), run_id, "auto-sensitive-call");
        let service = AgentService::new(Arc::clone(&storage)).with_builtin_capabilities(runtime);
        let context =
            AutoApprovedActionContext::new(input.clone(), run_id.to_string(), None, None, None);
        let cancellation = AgentCancellationToken::new();
        if cancelled {
            cancellation.cancel();
        }
        let result = service
            .execute_auto_builtin_mcp_tool_action(
                &context,
                Box::new(approval.clone()),
                cancellation,
            )
            .await
            .unwrap();
        assert_eq!(result.ok, !cancelled);
        assert_eq!(
            provider
                .invocations
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(!cancelled)
        );
        assert_eq!(
            provider
                .revocations
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(cancelled)
        );
        let durable = storage
            .get_pending_agent_action(&pending_action_storage_id(
                run_id,
                &approval.identity.action_id,
            ))
            .unwrap()
            .unwrap();
        assert_eq!(
            durable.status,
            if cancelled { "cancelled" } else { "completed" }
        );
        assert_eq!(durable.action_json, "{}");
        assert_eq!(durable.agent_input_json, "{}");

        let replay = service
            .execute_auto_builtin_mcp_tool_action(
                &context,
                Box::new(approval),
                AgentCancellationToken::new(),
            )
            .await;
        assert!(replay.is_err());
        assert_eq!(
            provider
                .invocations
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(!cancelled),
            "a durable terminal action must never invoke twice"
        );
    }
}

#[test]
fn builtin_sensitive_result_commit_failure_terminalizes_and_notifies_once() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "builtin-sensitive-commit-failure-run";
    let conversation_id = "builtin-sensitive-commit-failure-conversation";
    let assistant_message_id = "builtin-sensitive-commit-failure-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-commit-failure-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut record = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&storage_id)
        .cloned()
        .unwrap();
    service
        .transition_pending_status(&record, PendingActionStatus::Approved)
        .unwrap();
    record.snapshot.status = PendingActionStatus::Approved;
    service
        .transition_pending_status(&record, PendingActionStatus::Executing)
        .unwrap();
    record.snapshot.status = PendingActionStatus::Executing;

    let secret = "BUILTIN_EVALUATE_RESULT_MUST_NOT_REACH_ACTIVITY";
    let live_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({ "secret": secret })),
        error: None,
    };
    let mut persisted_input = record.agent_input.clone();
    persisted_input.approval_decision = Some(AgentApprovalDecision {
        action_id: action_id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    persisted_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: mycopilot_core::builtin_capability_tool_result_persistence_projection(&live_result),
    });
    let cancellation = AgentCancellationToken::new();
    service.register_cancellation(run_id, cancellation.clone());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    inject_manual_action_audit_failure(&record.storage_id, "completed");

    let _ = service.commit_builtin_mcp_tool_result_or_terminalize(
        &record,
        &persisted_input,
        PendingActionStatus::Completed,
        &notifications,
        &cancellation,
    );

    assert!(service.list_pending_actions().is_empty());
    assert!(!service
        .cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(run_id));
    let retired = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(retired.status, "failed");
    assert_eq!(retired.target_status.as_deref(), Some("failed"));
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["params"]["type"] == "tool_result")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["params"]["type"] == "done")
            .count(),
        1
    );
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(secret));
    assert!(serialized.contains("outcome_unknown"));
}

#[test]
fn builtin_sensitive_post_commit_error_adopts_receipt_and_emits_safe_result() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "builtin-sensitive-post-commit-run";
    let conversation_id = "builtin-sensitive-post-commit-conversation";
    let assistant_message_id = "builtin-sensitive-post-commit-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-post-commit-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut record = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&storage_id)
        .cloned()
        .unwrap();
    service
        .transition_pending_status(&record, PendingActionStatus::Approved)
        .unwrap();
    record.snapshot.status = PendingActionStatus::Approved;
    service
        .transition_pending_status(&record, PendingActionStatus::Executing)
        .unwrap();
    record.snapshot.status = PendingActionStatus::Executing;

    let secret = "BUILTIN_EVALUATE_LIVE_RESULT_MUST_BE_OMITTED";
    let live_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({ "secret": secret })),
        error: None,
    };
    let mut persisted_input = record.agent_input.clone();
    persisted_input.approval_decision = Some(AgentApprovalDecision {
        action_id,
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    persisted_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: mycopilot_core::builtin_capability_tool_result_persistence_projection(&live_result),
    });
    let cancellation = AgentCancellationToken::new();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    inject_manual_action_audit_post_commit_failure(&record.storage_id, "completed");

    let _ = service.commit_builtin_mcp_tool_result_or_terminalize(
        &record,
        &persisted_input,
        PendingActionStatus::Completed,
        &notifications,
        &cancellation,
    );
    service.emit_safe_builtin_mcp_tool_result(&notifications, run_id, &live_result);

    let pending = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, "executing");
    assert_eq!(pending.target_status.as_deref(), Some("completed"));
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["params"]["type"], "tool_result");
    assert_eq!(
        events[0]["params"]["result"]["result"]["status"],
        "completed"
    );
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(secret));
    service
        .transition_pending_status(&record, PendingActionStatus::Completed)
        .unwrap();
    assert_eq!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );
}

#[test]
fn builtin_sensitive_approval_tick_preserves_ticket_and_turn_ownership() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("builtin-sensitive-expiry-tick.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let expiry_clock_ms = mycopilot_core::storage::now_ms().saturating_add(901_000);
    let service =
        AgentService::new(Arc::clone(&storage)).with_mcp_approval_clock(move || expiry_clock_ms);
    let run_id = "builtin-sensitive-expiry-tick-run";
    let conversation_id = "builtin-sensitive-expiry-tick-conversation";
    let assistant_message_id = "builtin-sensitive-expiry-tick-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-expiry-tick-call",
    );
    let had_turn_permit = service
        .active_turn_permits
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(run_id);

    assert_eq!(service.list_pending_actions().len(), 1);
    assert_eq!(
        service
            .reconcile_expired_builtin_mcp_tool_approvals()
            .unwrap(),
        0
    );
    assert_eq!(service.list_pending_actions().len(), 1);
    assert_eq!(
        service
            .reconcile_expired_builtin_mcp_tool_approvals()
            .unwrap(),
        0,
        "ticket reconciliation remains a no-op"
    );
    assert!(service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
    assert_eq!(
        service
            .active_turn_permits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(run_id),
        had_turn_permit
    );

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let retained = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, "pending");
    assert_ne!(retained.action_json, "{}");
    assert_ne!(retained.agent_input_json, "{}");
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        0
    );
}

#[tokio::test]
async fn approving_an_expired_builtin_sensitive_action_accepts_a_normal_failed_result() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let expiry_clock_ms = mycopilot_core::storage::now_ms().saturating_add(901_000);
    let service =
        AgentService::new(Arc::clone(&storage)).with_mcp_approval_clock(move || expiry_clock_ms);
    let run_id = "builtin-sensitive-expiry-decision-run";
    let conversation_id = "builtin-sensitive-expiry-decision-conversation";
    let assistant_message_id = "builtin-sensitive-expiry-decision-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-expiry-decision-call",
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let output = service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
        .unwrap();
    assert_eq!(output.status, "failed");
    assert_eq!(output.agent_output.status, AgentRunStatus::Running);
    assert_eq!(
        output
            .tool_result
            .as_ref()
            .and_then(|result| result.result.as_ref())
            .and_then(|result| result.get("errorCode"))
            .and_then(Value::as_str),
        Some("mcp.tool_approval_payload_unavailable")
    );
    assert!(service.list_pending_actions().is_empty());

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let decided = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(decided.status, "approved");
    assert_eq!(decided.target_status.as_deref(), Some("failed"));

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    assert!(service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .is_err());
}

#[test]
fn builtin_sensitive_startup_terminalization_is_typed_atomic_and_secret_free() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("builtin-sensitive-startup.sqlite");
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
    let now_ms = mycopilot_core::storage::now_ms();
    let now_seconds = u64::try_from(now_ms).unwrap() / 1_000;
    let secret = "BUILTIN_STARTUP_RAW_ARGS_MUST_NEVER_PERSIST";
    let cases = [
        (
            "pending",
            McpStartupActionTerminalOutcome::PayloadUnavailable,
            "payload_unavailable",
            "definitely_not_dispatched",
        ),
        (
            "approved",
            McpStartupActionTerminalOutcome::PayloadUnavailable,
            "payload_unavailable",
            "definitely_not_dispatched",
        ),
        (
            "executing",
            McpStartupActionTerminalOutcome::OutcomeUnknown,
            "outcome_unknown",
            "possibly_dispatched",
        ),
        (
            "pending",
            McpStartupActionTerminalOutcome::Expired,
            "expired",
            "definitely_not_dispatched",
        ),
    ];

    for (index, (expected_status, outcome, result_status, certainty)) in
        cases.into_iter().enumerate()
    {
        let run_id = format!("builtin-sensitive-startup-run-{index}");
        let conversation_id = format!("builtin-sensitive-startup-conversation-{index}");
        let assistant_message_id = format!("builtin-sensitive-startup-assistant-{index}");
        let action_id = uuid::Uuid::new_v4().to_string();
        let call = AgentToolCall {
            id: format!("builtin-sensitive-startup-call-{index}"),
            tool: "browser_evaluate".to_string(),
            // The raw evaluate function/password/cookie/file handle is process-sealed. The
            // durable pending ToolCall is the exact private projection and contains no args.
            args: json!({}),
            approval_status: AgentApprovalStatus::Required,
            reason: Some("Run the reviewed page script.".to_string()),
        };
        let approval = mycopilot_core::AgentBuiltinMcpToolApproval {
            schema_version: mycopilot_core::BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION,
            identity: mycopilot_core::BuiltinMcpToolApprovalIdentity {
                action_id: action_id.clone(),
                approval_id: uuid::Uuid::new_v4().to_string(),
                run_id: run_id.clone(),
                call_id: call.id.clone(),
                capability_id: "browser_automation".to_string(),
                capability_activation_id: uuid::Uuid::new_v4().to_string(),
                managed_mcp_id: "builtin.browser_automation.mcp".to_string(),
                package_name: "@playwright/mcp".to_string(),
                package_version: "0.0.79".to_string(),
                upstream_catalog_digest: format!("sha256:{}", "1".repeat(64)),
                manifest_digest: format!("sha256:{}", "2".repeat(64)),
                policy_digest: format!("sha256:{}", "3".repeat(64)),
                policy_revision: 1,
                tool_id: "browser_evaluate".to_string(),
                raw_name: "browser_evaluate".to_string(),
                model_name: "browser_evaluate".to_string(),
                upstream_schema_digest: format!("sha256:{}", "4".repeat(64)),
                host_overlay_digest: format!("sha256:{}", "5".repeat(64)),
                host_input_schema_digest: format!("sha256:{}", "6".repeat(64)),
                arguments_digest: format!("sha256:{}", "7".repeat(64)),
                resource_scope_digest: format!("sha256:{}", "8".repeat(64)),
                origin: Some("https://mail.example.test".to_string()),
            },
            capability_display_name: "Browser automation".to_string(),
            tool_display_name: "Evaluate page script".to_string(),
            call_reason: "Run the reviewed page script.".to_string(),
            operation_category: "page_script_execution".to_string(),
            resource_summary: mycopilot_core::BuiltinMcpToolResourceSummary {
                scope: "page_script_execution".to_string(),
                display_name: "Current managed page".to_string(),
                file_basenames: Vec::new(),
                origin: Some("https://mail.example.test".to_string()),
            },
            risk_kinds: vec![mycopilot_core::BuiltinMcpToolRiskKind::PageScriptExecution],
            created_at: now_seconds,
            expires_at: now_seconds.saturating_add(900),
            approval_status: AgentApprovalStatus::Required,
        };
        let action = AgentProposedAction::BuiltinMcpToolApproval {
            approval: Box::new(approval),
        };
        let provenance = AgentToolIdentity::BuiltinCapability {
            capability_id: "browser_automation".into(),
            managed_mcp_id: "builtin.browser_automation.mcp".into(),
            package_name: "@playwright/mcp".into(),
            package_version: "0.0.79".into(),
            upstream_catalog_digest: format!("sha256:{}", "1".repeat(64)).into(),
            policy_digest: format!("sha256:{}", "3".repeat(64)).into(),
            manifest_digest: format!("sha256:{}", "2".repeat(64)).into(),
            tool_id: "browser_evaluate".into(),
            raw_name: "browser_evaluate".into(),
            model_name: "browser_evaluate".into(),
            upstream_schema_digest: format!("sha256:{}", "4".repeat(64)).into(),
            host_overlay_digest: format!("sha256:{}", "5".repeat(64)).into(),
            host_input_schema_digest: format!("sha256:{}", "6".repeat(64)).into(),
        };
        let mut input: AgentChatInput = serde_json::from_value(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "test-token",
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "messages": []
        }))
        .unwrap();
        freeze_test_pending_provider_configuration(&storage, &mut input);
        input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
            &storage,
            &run_id,
            Some(&action_id),
            &call,
            provenance.clone(),
        ));
        seed_durable_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call,
            provenance,
            now_ms.saturating_add(i64::try_from(index).unwrap()),
        );
        assert!(service
            .store_pending_action(
                &run_id,
                &conversation_id,
                &assistant_message_id,
                action,
                input,
            )
            .unwrap());
        let storage_id = pending_action_storage_id(&run_id, &action_id);
        if expected_status != "pending" {
            let connection = rusqlite::Connection::open(&database_path).unwrap();
            connection
                .execute(
                    "UPDATE agent_pending_actions SET status = ?2 WHERE action_id = ?1",
                    rusqlite::params![storage_id, expected_status],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE agent_action_audit SET status = ?2 WHERE action_id = ?1",
                    rusqlite::params![storage_id, expected_status],
                )
                .unwrap();
        }
        let prefix = storage
            .get_conversation_turn_trace(&assistant_message_id)
            .unwrap()
            .unwrap();
        let durable_row = storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap();
        let terminal_at = durable_row
            .created_at
            .max(durable_row.updated_at)
            .saturating_add(1_000);
        assert!(storage
            .terminalize_builtin_mcp_tool_agent_action_on_startup(
                &storage_id,
                expected_status,
                outcome,
                terminal_at,
            )
            .unwrap());
        assert!(!storage
            .terminalize_builtin_mcp_tool_agent_action_on_startup(
                &storage_id,
                expected_status,
                outcome,
                terminal_at.saturating_add(1),
            )
            .unwrap());
        let trace = storage
            .get_conversation_turn_trace(&assistant_message_id)
            .unwrap()
            .unwrap();
        assert_eq!(&trace.items[..prefix.items.len()], prefix.items.as_slice());
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
                ))
                .count(),
            1
        );
        let rendered = serde_json::to_string(&trace).unwrap();
        assert!(rendered.contains(&format!("\"status\":\"{result_status}\"")));
        assert!(rendered.contains(&format!("\"dispatchCertainty\":\"{certainty}\"")));
        assert!(!rendered.contains(secret));
        let retired = storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap();
        assert_eq!(retired.status, "failed");
        assert_eq!(retired.action_json, "{}");
        assert_eq!(retired.agent_input_json, "{}");
    }
}

#[test]
fn builtin_sensitive_rejection_crash_window_recovers_the_exact_rejected_receipt() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("builtin-sensitive-reject-crash.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "builtin-sensitive-reject-crash-run";
    let conversation_id = "builtin-sensitive-reject-crash-conversation";
    let assistant_message_id = "builtin-sensitive-reject-crash-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-reject-crash-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let record = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&storage_id)
        .unwrap()
        .clone();
    let AgentProposedAction::BuiltinMcpToolApproval { approval } = &record.snapshot.action else {
        panic!("expected the typed built-in MCP approval");
    };
    let feedback_secret =
        "使用密码 UNLABELLED_PASSWORD_CANARY_7Yp9；Use UNLABELLED_REJECTION_SECRET";
    let live_result =
        mycopilot_core::builtin_mcp_tool_rejected_result(approval, Some(feedback_secret));
    let mut persisted_input = record.agent_input.clone();
    persisted_input.approval_decision = Some(AgentApprovalDecision {
        action_id: action_id.clone(),
        status: AgentApprovalDecisionStatus::Rejected,
        message: None,
    });
    persisted_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: mycopilot_core::builtin_capability_tool_result_persistence_projection(&live_result),
    });
    let completed_at = mycopilot_core::storage::now_ms();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .commit_rejected_mcp_receipt(&record, &persisted_input, completed_at, &notifications)
        .unwrap();

    let crash_window = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(crash_window.status, "pending");
    assert_eq!(crash_window.target_status.as_deref(), Some("rejected"));
    let committed_prefix = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        committed_prefix
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    let prefix_json = serde_json::to_string(&committed_prefix).unwrap();
    assert!(prefix_json.contains("\"status\":\"rejected\""));
    assert!(prefix_json.contains("\"dispatchCertainty\":\"definitely_not_dispatched\""));
    assert!(!prefix_json.contains("payload_unavailable"));
    assert!(!prefix_json.contains("outcome_unknown"));
    assert!(!prefix_json.contains("UNLABELLED_PASSWORD_CANARY_7Yp9"));
    assert!(!prefix_json.contains("UNLABELLED_REJECTION_SECRET"));

    // Simulate a process crash after the atomic receipt but before the in-memory continuation
    // claim. Startup must adopt that exact receipt, never reinterpret refusal as dispatch.
    drop(service);
    let restarted = AgentService::new(Arc::clone(&storage));
    assert!(!restarted
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(&storage_id));
    let retired = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(retired.status, "rejected");
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        &trace.items[..committed_prefix.items.len()],
        committed_prefix.items.as_slice()
    );
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    let trace_json = serde_json::to_string(&trace).unwrap();
    assert!(!trace_json.contains("payload_unavailable"));
    assert!(!trace_json.contains("outcome_unknown"));
    assert!(!trace_json.contains("UNLABELLED_PASSWORD_CANARY_7Yp9"));
    assert!(!trace_json.contains("UNLABELLED_REJECTION_SECRET"));
}

#[test]
fn builtin_sensitive_cancel_reaches_pre_spawn_and_dispatching_process_guards() {
    for (index, target_status) in [
        PendingActionStatus::Approved,
        PendingActionStatus::Executing,
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        save_test_pending_provider(
            &storage,
            "test-model",
            "http://127.0.0.1:9/v1/chat/completions",
            "test-token",
            "disabled",
            "",
        );
        let service = AgentService::new(Arc::clone(&storage));
        let run_id = format!("builtin-sensitive-cancel-run-{index}");
        let conversation_id = format!("builtin-sensitive-cancel-conversation-{index}");
        let assistant_message_id = format!("builtin-sensitive-cancel-assistant-{index}");
        let (action_id, _) = store_builtin_sensitive_test_pending(
            &service,
            &storage,
            &run_id,
            &conversation_id,
            &assistant_message_id,
            &format!("builtin-sensitive-cancel-call-{index}"),
        );
        let storage_id = pending_action_storage_id(&run_id, &action_id);
        let record = service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&storage_id)
            .unwrap()
            .clone();
        service
            .transition_pending_status(&record, PendingActionStatus::Approved)
            .unwrap();
        if target_status == PendingActionStatus::Executing {
            let approved = service
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&storage_id)
                .unwrap()
                .clone();
            service
                .transition_pending_status(&approved, PendingActionStatus::Executing)
                .unwrap();
        }
        let guard = service.process_runs.register(&storage_id, &run_id);
        assert!(service.cancel_action(&run_id, &action_id).unwrap());
        assert!(guard
            .cancel_flag()
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            service
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&storage_id)
                .unwrap()
                .snapshot
                .status,
            target_status
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builtin_sensitive_reject_wins_approve_cancel_and_double_reject_races_once() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "builtin-sensitive-reject-race-run";
    let conversation_id = "builtin-sensitive-reject-race-conversation";
    let assistant_message_id = "builtin-sensitive-reject-race-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-reject-race-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let approve_entered = Arc::new(std::sync::Barrier::new(2));
    let approve_release = Arc::new(std::sync::Barrier::new(2));
    crate::application::agent::approval::install_approval_decision_barrier_hook(
        &action_id,
        AgentApprovalDecisionStatus::Approved,
        {
            let entered = Arc::clone(&approve_entered);
            let release = Arc::clone(&approve_release);
            Arc::new(move || {
                entered.wait();
                release.wait();
            })
        },
    );
    let approve = {
        let service = service.clone();
        let action_id = action_id.clone();
        let notifications = notifications.clone();
        tokio::spawn(async move {
            service.queue_action_continuation(
                run_id,
                &action_id,
                AgentApprovalDecisionStatus::Approved,
                None,
                notifications,
            )
        })
    };
    approve_entered.wait();
    let output = service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Rejected,
            Some("Use the public workflow instead.".to_string()),
            notifications.clone(),
        )
        .unwrap();
    assert_eq!(output.status, "rejected");
    approve_release.wait();
    assert!(approve.await.unwrap().is_err());
    assert!(service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Rejected,
            None,
            notifications,
        )
        .is_err());
    assert!(!service.cancel_action(run_id, &action_id).unwrap());
    let row = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "rejected");
    assert_eq!(row.target_status.as_deref(), Some("rejected"));
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builtin_sensitive_approve_claim_blocks_late_reject_and_settles_once() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "builtin-sensitive-approve-race-run";
    let conversation_id = "builtin-sensitive-approve-race-conversation";
    let assistant_message_id = "builtin-sensitive-approve-race-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-approve-race-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let reject_entered = Arc::new(std::sync::Barrier::new(2));
    let reject_release = Arc::new(std::sync::Barrier::new(2));
    crate::application::agent::approval::install_approval_decision_barrier_hook(
        &action_id,
        AgentApprovalDecisionStatus::Rejected,
        {
            let entered = Arc::clone(&reject_entered);
            let release = Arc::clone(&reject_release);
            Arc::new(move || {
                entered.wait();
                release.wait();
            })
        },
    );
    let reject = {
        let service = service.clone();
        let action_id = action_id.clone();
        let notifications = notifications.clone();
        tokio::spawn(async move {
            service.queue_action_continuation(
                run_id,
                &action_id,
                AgentApprovalDecisionStatus::Rejected,
                None,
                notifications,
            )
        })
    };
    reject_entered.wait();
    let approved = service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications.clone(),
        )
        .unwrap();
    assert_eq!(approved.status, "failed");
    assert_eq!(approved.agent_output.status, AgentRunStatus::Running);
    assert_eq!(
        approved
            .tool_result
            .as_ref()
            .and_then(|result| result.result.as_ref())
            .and_then(|result| result.get("errorCode"))
            .and_then(Value::as_str),
        Some("mcp.tool_approval_payload_unavailable")
    );
    reject_release.wait();
    assert!(reject.await.unwrap().is_err());

    for _ in 0..200 {
        if storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .is_some_and(|record| record.status == "failed")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );

    drop(service);
    let restarted = AgentService::new(Arc::clone(&storage));
    assert!(!restarted
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(&storage_id));
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    let rendered = serde_json::to_string(&trace).unwrap();
    assert!(rendered.contains("payload_unavailable"));
    assert!(rendered.contains("definitely_not_dispatched"));
    assert!(!rendered.contains("outcome_unknown"));
}

#[test]
fn pending_resume_sqlite_row_contains_only_versioned_secret_free_projection() {
    const API_TOKEN_CANARY: &str = "SQLITE_PENDING_API_TOKEN_CANARY_DO_NOT_PERSIST";
    const URL_CANARY: &str = "SQLITE_PENDING_URL_CANARY_DO_NOT_PERSIST";
    const SEARCH_CANARY: &str = "SQLITE_PENDING_SEARCH_CANARY_DO_NOT_PERSIST";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let sensitive_api_url = format!("https://example.test/{URL_CANARY}/v1");
    save_test_pending_provider(
        &storage,
        "test-model",
        &sensitive_api_url,
        API_TOKEN_CANARY,
        "tavily",
        SEARCH_CANARY,
    );
    let service = AgentService::new(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": sensitive_api_url.clone(),
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
    assert!(
        PersistedAgentResumeInput::decode(&row.agent_input_json).is_ok(),
        "the durable projection must use the current resume-input schema"
    );
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
        provider_model_id: "unrelated-restart-model".to_string(),
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
        persisted_endpoint_digest(&sensitive_api_url)
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
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        storage,
        "provider-resume-run",
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    );
    checkpoint.provider_profile_config = input.provider_profile_config.clone().unwrap();
    checkpoint.provider_protocol_key = input.provider_protocol_key.clone().unwrap();
    input.resume_checkpoint = Some(checkpoint);
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
fn pending_resume_selects_local_config_when_provider_model_id_is_shared() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let selected_endpoint = "https://selected-provider.example/v1";
    let selected_token = "selected-provider-token";
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://global-provider.example/v1",
        "global-provider-token",
        "disabled",
        "",
    );

    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].api_url_override = Some(selected_endpoint.to_string());
    settings.models[0].api_token_override = Some(selected_token.to_string());
    let decoy = ModelConfigRecord {
        id: "decoy-model-config".to_string(),
        provider_model_id: "test-model".to_string(),
        display_name: "Decoy model config".to_string(),
        api_url_override: Some("https://decoy-provider.example/v1".to_string()),
        api_token_override: Some("decoy-provider-token".to_string()),
        supports_image: false,
        context_window_tokens: Some(64_000),
        provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        input_price: "0".to_string(),
        cached_input_price: String::new(),
        output_price: "0".to_string(),
        enabled: true,
    };
    settings.models.insert(0, decoy);
    storage.save_model_settings(settings).unwrap();

    let frozen = frozen_provider_resume_input(
        &storage,
        selected_endpoint,
        selected_token,
        "disabled",
        None,
    );
    assert_eq!(
        frozen.agent_input.model_config_id.as_deref(),
        Some("test-model")
    );
    assert_eq!(frozen.agent_input.model, "test-model");

    let restored = restore_agent_input_secrets(&storage, frozen).unwrap();
    assert_eq!(restored.api_url, selected_endpoint);
    assert_eq!(restored.api_token, selected_token);
    assert_ne!(restored.api_url, "https://decoy-provider.example/v1");
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
        provider_model_id: "unrelated-model".to_string(),
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
    storage
        .save_model_settings_request(renderer_model_settings_update(
            &storage,
            current,
            mycopilot_core::storage::models::ProviderProfileUpdate::SelectGeneric,
        ))
        .unwrap();

    let current = storage.load_model_settings().unwrap().unwrap();
    assert_eq!(
        current.models[0].provider_profile_config.profile().id,
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
                content: String::new(),
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
    agent_input.model_config_id = Some("test-model-config".to_string());
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
        pause_reason: mycopilot_core::AgentRunCheckpointPauseReason::Approval,
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-skill-redaction".to_string(),
        pending_action_id: None,
        file_change_run_grant_ref: None,
        pending_file_observation: None,
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
async fn skill_script_worker_setup_failure_persists_receipt_and_runs_continuation() {
    const SCRIPT_URI: &str = "skill://package/installed%3Auser%3Afixture/revision/scripts/build.py";
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let run_id = "skill-worker-setup-failure-run";
    let conversation_id = "skill-worker-setup-failure-conversation";
    let assistant_message_id = "skill-worker-setup-failure-assistant";
    let call = AgentToolCall {
        id: "skill-worker-setup-failure-call".to_string(),
        tool: "skills_run_script".to_string(),
        args: json!({ "scriptUri": SCRIPT_URI }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::SkillScript {
        script: Box::new(AgentSkillScriptRequest {
            id: call.id.clone(),
            script_uri: SCRIPT_URI.to_string(),
            skill_id: "installed:user:fixture".to_string(),
            skill_revision: "revision".to_string(),
            resource_path: "scripts/build.py".to_string(),
            resource_digest: "sha256:fixture".to_string(),
            source: mycopilot_core::AgentSkillScriptSourceProof {
                source_id: "installed:user".to_string(),
                source_kind: mycopilot_core::AgentSkillScriptSourceKind::Installed,
                trust: mycopilot_core::AgentSkillScriptTrust::Untrusted,
            },
            interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
            args: Vec::new(),
            requirements: mycopilot_core::AgentSkillScriptRequirements::default(),
            preflight: mycopilot_core::AgentSkillScriptPreflightReport {
                status: mycopilot_core::AgentSkillScriptPreflightStatus::Ready,
                interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
                interpreter_version: Some("Python 3".to_string()),
                dependencies: Vec::new(),
                runtime_fingerprint: "fixture-runtime".to_string(),
                error_code: None,
                message: None,
            },
            timeout_ms: Some(mycopilot_core::skills::DEFAULT_SKILL_SCRIPT_TIMEOUT_MS),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        }),
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "http://127.0.0.1:1/v1/chat/completions",
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
        AgentToolIdentity::Builtin {
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
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Executing)
        .unwrap();
    let executing = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    // The closed loopback endpoint fails deterministically after the Skill result receipt commits,
    // proving the common runner exit enters the real continuation without a paid Provider.
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
    let setup_failure = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result: Some(json!({
            "type": "skill_script",
            "status": "failed",
            "outcome": "definitely_not_executed",
            "code": "executionSetupFailed",
            "effectsMayHaveOccurred": false,
        })),
        error: Some("fixture Skill worker setup failed".to_string()),
    };
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .finish_skill_script_execution(
            executing,
            call.clone(),
            setup_failure,
            notifications,
            None,
            None,
        )
        .await;
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();

    let (status, target_status): (String, Option<String>) =
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(status, "failed", "events: {events:#?}");
    assert_eq!(target_status.as_deref(), Some("failed"));
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
    )));
    assert!(!service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
    assert!(events
        .iter()
        .any(|event| event["params"]["type"] == "error"));
}

#[derive(Clone, Copy)]
enum InjectedSkillScriptWorkerPanic {
    AfterEffectBoundary,
    AfterDurableReceipt,
}

async fn assert_queued_skill_script_worker_panic_is_supervised(
    injected: InjectedSkillScriptWorkerPanic,
) {
    const SCRIPT_URI: &str = "skill://package/installed%3Auser%3Afixture/revision/scripts/build.py";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let (run_id, conversation_id, assistant_message_id, call_id) = match injected {
        InjectedSkillScriptWorkerPanic::AfterEffectBoundary => (
            "skill-worker-supervised-panic-run",
            "skill-worker-supervised-panic-conversation",
            "skill-worker-supervised-panic-assistant",
            "skill-worker-supervised-panic-call",
        ),
        InjectedSkillScriptWorkerPanic::AfterDurableReceipt => (
            "skill-worker-post-receipt-panic-run",
            "skill-worker-post-receipt-panic-conversation",
            "skill-worker-post-receipt-panic-assistant",
            "skill-worker-post-receipt-panic-call",
        ),
    };
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: "skills_run_script".to_string(),
        args: json!({ "scriptUri": SCRIPT_URI }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::SkillScript {
        script: Box::new(AgentSkillScriptRequest {
            id: call.id.clone(),
            script_uri: SCRIPT_URI.to_string(),
            skill_id: "installed:user:fixture".to_string(),
            skill_revision: "revision".to_string(),
            resource_path: "scripts/build.py".to_string(),
            resource_digest: "sha256:fixture".to_string(),
            source: mycopilot_core::AgentSkillScriptSourceProof {
                source_id: "installed:user".to_string(),
                source_kind: mycopilot_core::AgentSkillScriptSourceKind::Installed,
                trust: mycopilot_core::AgentSkillScriptTrust::Untrusted,
            },
            interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
            args: Vec::new(),
            requirements: mycopilot_core::AgentSkillScriptRequirements::default(),
            preflight: mycopilot_core::AgentSkillScriptPreflightReport {
                status: mycopilot_core::AgentSkillScriptPreflightStatus::Ready,
                interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
                interpreter_version: Some("Python 3".to_string()),
                dependencies: Vec::new(),
                runtime_fingerprint: "fixture-runtime".to_string(),
                error_code: None,
                message: None,
            },
            timeout_ms: Some(mycopilot_core::skills::DEFAULT_SKILL_SCRIPT_TIMEOUT_MS),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        }),
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "http://127.0.0.1:1/v1/chat/completions",
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
        AgentToolIdentity::Builtin {
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
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Executing)
        .unwrap();
    let executing = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
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
    match injected {
        InjectedSkillScriptWorkerPanic::AfterEffectBoundary => {
            service.inject_skill_script_worker_panic_once(&call.id)
        }
        InjectedSkillScriptWorkerPanic::AfterDurableReceipt => {
            service.inject_skill_script_post_receipt_panic_once(&call.id)
        }
    }
    let process_guard = service.process_runs.register(&storage_id, run_id);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let queued = service
        .queue_skill_script_execution(executing, call.clone(), process_guard, notifications)
        .unwrap();
    assert_eq!(queued.status, "approved");
    assert_eq!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap()
            .status,
        "executing"
    );

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let current = storage
                .get_pending_agent_action(&storage_id)
                .unwrap()
                .unwrap();
            let trace_terminal = storage
                .get_conversation_turn_trace(assistant_message_id)
                .unwrap()
                .is_some_and(|trace| {
                    trace.terminal_status == ConversationTurnTraceTerminalStatus::Failed
                });
            if current.status == "failed"
                && current.target_status.as_deref() == Some("failed")
                && trace_terminal
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the supervised panic must converge without an executing ghost");

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    let observation = trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolResult {
                call_id,
                observation,
                ..
            } if call_id == &call.id => Some(observation),
            _ => None,
        })
        .expect("the panic outcome must have a durable ToolResult receipt");
    if matches!(
        injected,
        InjectedSkillScriptWorkerPanic::AfterEffectBoundary
    ) {
        assert_eq!(observation["status"], "outcome_unknown");
        assert_eq!(observation["outcome"], "outcome_unknown");
        assert_eq!(observation["effectsMayHaveOccurred"], true);
        assert_eq!(observation["retryable"], false);
    }
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    assert!(service
        .file_effects
        .unsettled_effect_ids_for_conversation(conversation_id)
        .is_empty());
    assert!(service
        .file_effects
        .active_run_ids_for_conversation(conversation_id)
        .is_empty());
    assert!(!service.process_runs.has_active_run(run_id));
    assert!(!service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
}

#[tokio::test]
async fn queued_skill_script_worker_panic_is_supervised_to_durable_outcome_unknown() {
    assert_queued_skill_script_worker_panic_is_supervised(
        InjectedSkillScriptWorkerPanic::AfterEffectBoundary,
    )
    .await;
}

#[tokio::test]
async fn queued_skill_script_post_receipt_panic_terminalizes_without_replay() {
    assert_queued_skill_script_worker_panic_is_supervised(
        InjectedSkillScriptWorkerPanic::AfterDurableReceipt,
    )
    .await;
}

#[tokio::test]
async fn projected_child_skill_approval_atomically_resumes_wake_before_worker_runs() {
    const SCRIPT_URI: &str =
        "skill://package/installed%3Auser%3Aapproval-fixture/revision/scripts/build.py";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:1/v1/chat/completions",
        "secret",
        "disabled",
        "",
    );
    let root_conversation_id = "projected-skill-approval-root-conversation";
    let root_agent_id = "projected-skill-approval-root-agent";
    storage
        .save_conversation(ChatConversationRecord {
            id: root_conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Projected Skill approval root".to_string(),
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
            agent_id: root_agent_id.to_string(),
            conversation_id: root_conversation_id.to_string(),
            creation_request_id: "projected-skill-approval-root-ensure".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    seed_root_effective_permissions(
        &storage,
        root_agent_id,
        root_conversation_id,
        "projected-skill-approval",
        AgentPermissions::default(),
    );
    let child = storage
        .create_child_agent(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: root_agent_id.to_string(),
            creation_request_id: "projected-skill-approval-child-spawn".to_string(),
            task_name: "projected_skill_approval_child".to_string(),
            task: "Wait for Skill script approval.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();
    let admitted_at = mycopilot_core::storage::now_ms();
    let claimed = storage
        .claim_next_dispatchable_agent_wake_at("projected-skill-approval-host", admitted_at)
        .unwrap()
        .unwrap();
    let run_id = "projected-skill-approval-run";
    let assistant_message_id = "projected-skill-approval-assistant";
    let call = AgentToolCall {
        id: "projected-skill-approval-call".to_string(),
        tool: "skills_run_script".to_string(),
        args: json!({ "scriptUri": SCRIPT_URI }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::SkillScript {
        script: Box::new(AgentSkillScriptRequest {
            id: call.id.clone(),
            script_uri: SCRIPT_URI.to_string(),
            skill_id: "installed:user:approval-fixture".to_string(),
            skill_revision: "revision".to_string(),
            resource_path: "scripts/build.py".to_string(),
            resource_digest: "sha256:fixture".to_string(),
            source: mycopilot_core::AgentSkillScriptSourceProof {
                source_id: "installed:user".to_string(),
                source_kind: mycopilot_core::AgentSkillScriptSourceKind::Installed,
                trust: mycopilot_core::AgentSkillScriptTrust::Untrusted,
            },
            interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
            args: Vec::new(),
            requirements: mycopilot_core::AgentSkillScriptRequirements::default(),
            preflight: mycopilot_core::AgentSkillScriptPreflightReport {
                status: mycopilot_core::AgentSkillScriptPreflightStatus::Ready,
                interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
                interpreter_version: Some("Python 3".to_string()),
                dependencies: Vec::new(),
                runtime_fingerprint: "fixture-runtime".to_string(),
                error_code: None,
                message: None,
            },
            timeout_ms: Some(mycopilot_core::skills::DEFAULT_SKILL_SCRIPT_TIMEOUT_MS),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        }),
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "http://127.0.0.1:1/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
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
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    );
    checkpoint.run_context = Some(context);
    checkpoint.collaboration_run_snapshot =
        Some(mycopilot_core::AgentCollaborationRunSnapshot::default());
    agent_input.resume_checkpoint = Some(checkpoint.clone());

    let (conversation, revision) = storage
        .load_conversation_for_turn(&child.agent.conversation_id)
        .unwrap();
    let mut conversation = conversation.unwrap();
    let waiting_run = json!({
        "runId": run_id,
        "status": "waiting_for_approval",
        "startedAt": admitted_at + 1,
        "toolDefinitions": [],
        "toolCalls": [{
            "id": call.id,
            "tool": call.tool,
            "args": call.args,
            "approvalStatus": "required",
            "reason": null
        }],
        "toolResults": [],
        "approvals": [serde_json::to_value(&action).unwrap()],
        "fileChangeProposals": [],
        "fileChanges": [],
        "webSearchActivities": [],
        "readActivities": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {},
        "timeline": [],
        "state": {
            "status": "waiting_for_approval",
            "activeRunId": run_id,
            "lastError": null,
            "updatedAt": admitted_at + 1
        }
    });
    conversation.messages.push(ChatMessageRecord {
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: String::new(),
        created_at: admitted_at + 1,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: Some(waiting_run.to_string()),
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
        items: checkpoint.conversation_trace_items.clone(),
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
            mycopilot_core::AgentTurnPermissionSource::InheritTrustedAncestors,
            &[claimed.source_agent_message_id.clone().unwrap()],
            &trace,
            admitted_at + 1,
            admitted_at + 1,
        )
        .unwrap();
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &checkpoint.conversation_model_context_items,
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
    storage
        .upsert_agent_usage(AgentUsageRecordInsert {
            id: "projected-skill-approval-usage".to_string(),
            conversation_id: child.agent.conversation_id.clone(),
            message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            project_id: None,
            model_id: "test-model".to_string(),
            model_name: "Test Model".to_string(),
            started_at: Some(admitted_at + 1),
            completed_at: None,
            status: Some("waiting_for_approval".to_string()),
            error: None,
            created_at: admitted_at + 1,
            input_tokens: None,
            output_tokens: None,
            output_thinking_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: 0,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            estimated_cost: None,
        })
        .unwrap();

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
            action,
            agent_input,
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    service.inject_skill_script_post_receipt_panic_once(&call.id);
    let decision = service
        .decide_root_projected_approval(
            root_conversation_id,
            &storage_id,
            ProjectedApprovalDecision::Approve,
            None,
            notifications,
        )
        .unwrap();

    // This current-thread Tokio test has not yielded since queueing the worker, so these facts
    // prove the approval bridge itself committed before any Skill execution could settle them.
    assert!(decision.accepted);
    assert_eq!(decision.status, "approved");
    let pending = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, "executing");
    let resumed = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
    assert_eq!(resumed.status, mycopilot_core::AgentWakeStatus::Running);
    assert_eq!(resumed.status_revision, waiting.status_revision + 1);
    let observer = storage
        .load_conversation_observer_snapshot(&child.agent.conversation_id)
        .unwrap()
        .unwrap();
    let assistant = observer
        .conversation
        .messages
        .iter()
        .find(|message| message.id == assistant_message_id)
        .unwrap();
    assert_eq!(assistant.status.as_deref(), Some("pending"));
    let projected_run: serde_json::Value =
        serde_json::from_str(assistant.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(projected_run["status"], "running");
    assert_eq!(projected_run["state"]["status"], "running");
    assert_eq!(projected_run["state"]["activeRunId"], run_id);
    assert_eq!(projected_run["toolCalls"][0]["approvalStatus"], "approved");
    assert_eq!(
        projected_run["approvals"][0]["script"]["approvalStatus"],
        "approved"
    );
    let usage = storage
        .load_agent_usage_for_owner(run_id, &child.agent.conversation_id, assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(usage.status.as_deref(), Some("running"));
    assert_eq!(
        storage
            .get_agent_display_status(&child.agent.agent_id)
            .unwrap()
            .status,
        mycopilot_core::AgentDisplayStatus::Running
    );
    assert!(service
        .list_root_projected_approvals(root_conversation_id)
        .unwrap()
        .is_empty());

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let pending = storage
                .get_pending_agent_action(&storage_id)
                .unwrap()
                .unwrap();
            let wake = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
            let trace_is_failed = storage
                .get_conversation_turn_trace(assistant_message_id)
                .unwrap()
                .is_some_and(|trace| {
                    trace.terminal_status == ConversationTurnTraceTerminalStatus::Failed
                });
            if pending.status == "failed"
                && pending.target_status.as_deref() == Some("failed")
                && wake.status == mycopilot_core::AgentWakeStatus::Failed
                && trace_is_failed
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("post-receipt panic must settle the child Turn without restart");
    let settled_wake = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
    assert_eq!(settled_wake.status, mycopilot_core::AgentWakeStatus::Failed);
    assert!(settled_wake.completed_at.is_some());
    assert!(settled_wake.result_message_id.is_some());
    assert!(service
        .file_effects
        .unsettled_effect_ids_for_conversation(&child.agent.conversation_id)
        .is_empty());
    assert!(service
        .file_effects
        .active_run_ids_for_conversation(&child.agent.conversation_id)
        .is_empty());
    assert!(!service.process_runs.has_active_run(run_id));
    assert!(!service
        .has_conversation_turn_occupancy(&child.agent.conversation_id)
        .unwrap());
    assert_eq!(
        storage
            .get_agent_display_status(&child.agent.agent_id)
            .unwrap()
            .status,
        mycopilot_core::AgentDisplayStatus::LatestFailed
    );
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
    seed_root_effective_permissions(
        &storage,
        "agent-child-approval-root",
        "conversation-child-approval-root",
        "child-approval-root",
        AgentPermissions::default(),
    );
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
        tool: "todo_update".to_string(),
        args: json!({ "items": [] }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let provenance = AgentToolIdentity::RuntimeExtension {
        extension_id: "todo".to_string(),
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
            mycopilot_core::AgentTurnPermissionSource::InheritTrustedAncestors,
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
            result: Some(json!({ "revision": 1, "items": [], "updatedAt": admitted_at + 1 })),
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
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
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
    match tokio::time::timeout(Duration::from_secs(2), provider_entered).await {
        Ok(entered) => entered.unwrap(),
        Err(error) => {
            let mut events = Vec::new();
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
            panic!(
                "continuation must reach the deterministic Provider boundary: {error:?}; events={events:#?}"
            );
        }
    }
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

#[tokio::test]
async fn pre_spawn_cancelled_continuation_retries_real_pending_target_and_releases_resources() {
    const RUN_ID: &str = "pre-spawn-cancelled-continuation-run";
    const CONVERSATION_ID: &str = "pre-spawn-cancelled-continuation-conversation";
    const ASSISTANT_MESSAGE_ID: &str = "pre-spawn-cancelled-continuation-assistant";

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    service
        .reserve_conversation_turn(CONVERSATION_ID, RUN_ID, ASSISTANT_MESSAGE_ID)
        .unwrap();
    service.ensure_turn_concurrency_permit(RUN_ID).unwrap();
    let gate = service.turn_concurrency_gate();
    assert_eq!(gate.active(), 1);

    let call = AgentToolCall {
        id: "pre-spawn-cancelled-continuation-call".to_string(),
        tool: "todo_update".to_string(),
        args: json!({ "items": [] }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let provenance = AgentToolIdentity::RuntimeExtension {
        extension_id: "todo".to_string(),
        tool_name: call.tool.clone(),
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
        RUN_ID,
        None,
        &call,
        provenance.clone(),
    ));
    let run_context = AgentRunContext {
        conversation_id: Some(CONVERSATION_ID.to_string()),
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
        CONVERSATION_ID,
        ASSISTANT_MESSAGE_ID,
        RUN_ID,
        &call,
        provenance,
        1,
    );
    service
        .store_pending_action(
            RUN_ID,
            CONVERSATION_ID,
            ASSISTANT_MESSAGE_ID,
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input.clone(),
        )
        .unwrap();
    let storage_id = pending_action_storage_id(RUN_ID, &call.id);
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

    let usage = AgentUsage {
        input_tokens: Some(20),
        output_tokens: Some(8),
        output_thinking_tokens: Some(3),
        total_tokens: Some(28),
        cached_input_tokens: Some(2),
        cache_creation_input_tokens: None,
        billable_request_count: Some(1),
    };
    service.register_usage_context(
        RUN_ID,
        AgentRunUsageContext {
            conversation_id: CONVERSATION_ID.to_string(),
            assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
            run_id: RUN_ID.to_string(),
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
    service
        .persist_run_usage(
            RUN_ID,
            AgentRunStatus::WaitingForApproval,
            Some(usage.clone()),
            None,
        )
        .unwrap();

    let mut resumed_input = agent_input;
    resumed_input.approval_decision = Some(AgentApprovalDecision {
        action_id: call.id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    resumed_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({ "revision": 1, "items": [], "updatedAt": 1 })),
            error: None,
        },
    });
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .commit_trace_snapshot_with_continuation(&approved, &resumed_input, &notifications)
        .unwrap();

    let cancellation = AgentCancellationToken::new();
    service.register_cancellation(RUN_ID, cancellation.clone());
    cancellation.cancel();
    inject_pending_status_transition_failure(&storage_id, "completed");
    service
        .run_action_continuation(
            approved,
            resumed_input,
            notifications,
            PendingActionStatus::Completed,
            Some(cancellation),
        )
        .await;

    let (pending_status, pending_target_status): (String, String) =
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(pending_status, "completed");
    assert_eq!(pending_target_status, "completed");
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())[&storage_id]
            .snapshot
            .status,
        PendingActionStatus::Completed
    );

    let trace = storage
        .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert!(trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
    )));
    let persisted_usage = storage
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(persisted_usage.status.as_deref(), Some("cancelled"));
    assert_eq!(persisted_usage.total_tokens, usage.total_tokens);
    assert_eq!(persisted_usage.billable_request_count, 1);
    assert!(!service
        .has_conversation_turn_occupancy(CONVERSATION_ID)
        .unwrap());
    assert_eq!(gate.active(), 0);
    assert!(!service
        .active_turn_permits
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(RUN_ID));
    assert!(!service
        .usage_contexts
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(RUN_ID));
    assert!(!service
        .cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(RUN_ID));

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(!events
        .iter()
        .any(|event| event["params"]["type"] == "error"));
    let done = events
        .iter()
        .filter(|event| event["params"]["type"] == "done")
        .collect::<Vec<_>>();
    assert_eq!(done.len(), 1, "events: {events:#?}");
    assert_eq!(done[0]["params"]["status"], "cancelled");
    assert_eq!(done[0]["params"]["usage"]["totalTokens"], 28);
    assert_eq!(done[0]["params"]["usage"]["billableRequestCount"], 1);
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
        pause_reason: mycopilot_core::AgentRunCheckpointPauseReason::Approval,
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-invalid-checkpoint".to_string(),
        pending_action_id: None,
        file_change_run_grant_ref: None,
        pending_file_observation: None,
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
                content: String::new(),
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
    assert!(conversation.messages[0].content.is_empty());
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    assert!(storage
        .list_agent_tool_results_for_run("run-cancel-usage-failure", "approval_tool")
        .unwrap()
        .is_empty());
}

#[test]
fn cancelled_file_change_with_durable_abort_never_rolls_back_to_pending() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-file-change-cancel-failure".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "FileChange cancel failure".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-file-change-cancel-failure".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
        "run-file-change-cancel-failure",
        AgentRunUsageContext {
            conversation_id: "missing-file-change-usage-owner".to_string(),
            assistant_message_id: "assistant-file-change-cancel-failure".to_string(),
            run_id: "run-file-change-cancel-failure".to_string(),
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
    let action_id = "file-change-cancel-failure";
    let canonical_target = fixture.path().join("cancelled.txt");
    let (file_change, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id: "run-file-change-cancel-failure",
            conversation_id: "conversation-file-change-cancel-failure",
            call_id: action_id,
            transaction_id: "transaction-file-change-cancel-failure",
        },
        ("cancelled.txt", canonical_target.to_str().unwrap()),
        "hello",
        AgentApprovalStatus::Required,
    );
    storage
        .create_agent_file_change(staged_file_change_record(&file_change, "waiting_approval"))
        .unwrap();
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    let run_context = AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-file-change-cancel-failure".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
    };
    agent_input.context = Some(run_context.clone());
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        &storage,
        "run-file-change-cancel-failure",
        Some(&pending_action_storage_id(
            "run-file-change-cancel-failure",
            action_id,
        )),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    );
    checkpoint.run_context = Some(run_context);
    agent_input.resume_checkpoint = Some(checkpoint);
    append_durable_pending_trace(
        &storage,
        "conversation-file-change-cancel-failure",
        "assistant-file-change-cancel-failure",
        "run-file-change-cancel-failure",
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            "run-file-change-cancel-failure",
            "conversation-file-change-cancel-failure",
            "assistant-file-change-cancel-failure",
            AgentProposedAction::FileChange { file_change },
            agent_input,
        )
        .unwrap();

    let error = service
        .cancel_action("run-file-change-cancel-failure", action_id)
        .unwrap_err();
    assert!(
        error.contains("保持 executing"),
        "unexpected error: {error}"
    );
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|lock_error| lock_error.into_inner())
            [&pending_action_storage_id("run-file-change-cancel-failure", action_id,)]
            .snapshot
            .status,
        PendingActionStatus::Executing
    );
    assert_eq!(
        storage
            .get_agent_file_change("transaction-file-change-cancel-failure")
            .unwrap()
            .unwrap()
            .status,
        "aborted"
    );
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-file-change-cancel-failure")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    let conversation = storage
        .load_conversation("conversation-file-change-cancel-failure")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    let reconciled_at = mycopilot_core::storage::now_ms();
    assert_eq!(
        reconcile_interrupted_file_changes(&storage, reconciled_at).unwrap(),
        1,
        "startup pre-pass must rebuild the exact aborted ToolResult before terminalizing"
    );
    let recovered_trace = storage
        .get_conversation_turn_trace("assistant-file-change-cancel-failure")
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered_trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. }
                    if call_id == action_id
            ))
            .count(),
        1
    );
    let recovered_audit = storage
        .get_agent_action_audit(&pending_action_storage_id(
            "run-file-change-cancel-failure",
            action_id,
        ))
        .unwrap()
        .unwrap();
    let recovered_result: AgentFileChangeResult = serde_json::from_str(
        recovered_audit
            .file_change_result_json
            .as_deref()
            .expect("cancel recovery persists its typed FileChange result"),
    )
    .unwrap();
    assert_eq!(
        recovered_result.status,
        mycopilot_core::AgentFileChangeResultStatus::Aborted
    );
    assert_eq!(
        recovered_result.outcome,
        mycopilot_core::AgentFileChangeOutcome::DefinitelyNotExecuted
    );
    let interrupted = storage
        .reconcile_interrupted_pending_agent_actions(reconciled_at)
        .unwrap();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].status, "executing");
    assert_eq!(interrupted[0].target_status.as_deref(), Some("cancelled"));
    assert!(storage.list_pending_agent_actions().unwrap().is_empty());
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-file-change-cancel-failure")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
}
